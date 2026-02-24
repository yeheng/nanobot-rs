//! History context processor for managing conversation history
//!
//! This module provides a flexible framework for processing conversation history
//! before it is sent to the LLM. It supports multiple strategies:
//!
//! - **Direct Inject**: Include all history without modification
//! - **Truncate**: Simple character-based truncation
//! - **Token Budget**: Token-aware context window management
//! - **Summarize**: Compress old messages into summaries
//! - **Relevance Filter**: Select messages based on relevance to current input

use std::sync::Arc;

use crate::providers::LlmProvider;
use crate::session::SessionMessage;

/// Configuration for history processing
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Maximum number of messages to include
    pub max_messages: usize,
    /// Token budget for history (0 = unlimited)
    pub token_budget: usize,
    /// Number of recent messages to keep verbatim
    pub recent_keep: usize,
    /// Maximum characters for truncated messages
    pub truncate_length: usize,
    /// Enable relevance filtering
    pub enable_relevance_filter: bool,
    /// Enable summarization of old messages
    pub enable_summarization: bool,
    /// Minimum messages before summarization kicks in
    pub summarize_threshold: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_messages: 50,
            token_budget: 8000, // ~8k tokens for context window
            recent_keep: 10,
            truncate_length: 100,
            enable_relevance_filter: false,
            enable_summarization: false,
            summarize_threshold: 20,
        }
    }
}

/// Result of processing history
#[derive(Debug, Clone)]
pub struct ProcessedHistory {
    /// The processed messages
    pub messages: Vec<SessionMessage>,
    /// Estimated token count
    pub estimated_tokens: usize,
    /// Number of messages that were filtered out
    pub filtered_count: usize,
    /// Number of messages that were summarized
    pub summarized_count: usize,
}

/// Trait for history processing strategies
///
/// This allows for pluggable strategies to handle conversation history
/// before it's sent to the LLM. Different use cases may require different
/// approaches:
///
/// - Simple chat: use `DirectInjectStrategy`
/// - Long conversations: use `TokenBudgetStrategy` or `SummarizeStrategy`
/// - Context-aware responses: use `RelevanceFilterStrategy`
pub trait HistoryStrategy: Send + Sync {
    /// Process the given history according to this strategy
    fn process(
        &self,
        history: Vec<SessionMessage>,
        current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory;

    /// Get the name of this strategy
    fn name(&self) -> &str;

    /// Get a description of this strategy
    fn description(&self) -> &str;
}

/// Strategy that includes all history without modification
///
/// This is the simplest strategy, useful when context window is not a concern
/// or when you want to pass all context to the LLM.
pub struct DirectInjectStrategy;

impl HistoryStrategy for DirectInjectStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        _current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        let messages: Vec<SessionMessage> = history.into_iter().take(config.max_messages).collect();
        let estimated_tokens = Self::estimate_tokens(&messages);

        ProcessedHistory {
            messages,
            estimated_tokens,
            filtered_count: 0,
            summarized_count: 0,
        }
    }

    fn name(&self) -> &str {
        "direct"
    }

    fn description(&self) -> &str {
        "Include all history without modification"
    }
}

impl DirectInjectStrategy {
    fn estimate_tokens(messages: &[SessionMessage]) -> usize {
        // Rough estimate: ~4 characters per token for English, ~2 for Chinese
        // Using 3 as a reasonable middle ground
        messages.iter().map(|m| m.content.len() / 3).sum()
    }
}

/// Strategy that truncates older messages to save space
///
/// This is the default strategy, providing a balance between
/// preserving context and managing token usage.
pub struct TruncateStrategy {
    /// Number of recent messages to keep verbatim
    recent_keep: usize,
    /// Maximum characters for truncated messages
    truncate_length: usize,
}

impl Default for TruncateStrategy {
    fn default() -> Self {
        Self {
            recent_keep: 10,
            truncate_length: 100,
        }
    }
}

impl HistoryStrategy for TruncateStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        _current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        let total = history.len().min(config.max_messages);
        let trim_boundary = total.saturating_sub(self.recent_keep);
        let mut messages = Vec::with_capacity(total);

        for (i, msg) in history.into_iter().take(config.max_messages).enumerate() {
            if i < trim_boundary {
                // Truncate: keep only first N chars
                let truncated_content = truncate_content(&msg.content, self.truncate_length);
                messages.push(SessionMessage {
                    role: msg.role,
                    content: truncated_content,
                    timestamp: msg.timestamp,
                    tools_used: msg.tools_used,
                });
            } else {
                messages.push(msg);
            }
        }

        let estimated_tokens = Self::estimate_tokens(&messages);

        ProcessedHistory {
            messages,
            estimated_tokens,
            filtered_count: 0,
            summarized_count: 0,
        }
    }

    fn name(&self) -> &str {
        "truncate"
    }

    fn description(&self) -> &str {
        "Truncate older messages to save space"
    }
}

impl TruncateStrategy {
    fn estimate_tokens(messages: &[SessionMessage]) -> usize {
        messages.iter().map(|m| m.content.len() / 3).sum()
    }
}

/// Strategy that manages context within a token budget
///
/// This strategy ensures that the total token count of history
/// stays within a specified budget, trimming from the oldest
/// messages if necessary.
pub struct TokenBudgetStrategy {
    /// Token budget for history
    token_budget: usize,
    /// Number of recent messages to always keep
    protected_recent: usize,
}

impl TokenBudgetStrategy {
    /// Create a new token budget strategy
    pub fn new(token_budget: usize, protected_recent: usize) -> Self {
        Self {
            token_budget,
            protected_recent,
        }
    }

    fn count_tokens(text: &str) -> usize {
        // Simple token estimation: ~3 characters per token
        text.len() / 3
    }

    fn estimate_tokens(messages: &[SessionMessage]) -> usize {
        messages
            .iter()
            .map(|m| Self::count_tokens(&m.content))
            .sum()
    }
}

impl Default for TokenBudgetStrategy {
    fn default() -> Self {
        Self {
            token_budget: 8000,
            protected_recent: 5,
        }
    }
}

impl HistoryStrategy for TokenBudgetStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        _current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        let mut messages: Vec<SessionMessage> =
            history.into_iter().take(config.max_messages).collect();
        let total = messages.len();

        // Separate protected (recent) and unprotected messages
        let protected_start = total.saturating_sub(self.protected_recent);
        let mut protected: Vec<SessionMessage> = messages.split_off(protected_start);

        // Calculate current token usage
        let mut current_tokens: usize = protected
            .iter()
            .map(|m| Self::count_tokens(&m.content))
            .sum();

        // Process unprotected messages, starting from the most recent
        // (we want to keep more recent messages, drop older ones)
        let mut kept_unprotected: Vec<SessionMessage> = Vec::new();

        for msg in messages.into_iter().rev() {
            let msg_tokens = Self::count_tokens(&msg.content);

            if current_tokens + msg_tokens <= self.token_budget {
                current_tokens += msg_tokens;
                kept_unprotected.push(msg);
            }
            // else: skip this message due to budget
        }

        // Reorder: older messages first
        kept_unprotected.reverse();

        // Combine: unprotected + protected
        let mut all_messages = kept_unprotected;
        all_messages.append(&mut protected);

        let filtered_count = total - all_messages.len();

        ProcessedHistory {
            messages: all_messages,
            estimated_tokens: current_tokens,
            filtered_count,
            summarized_count: 0,
        }
    }

    fn name(&self) -> &str {
        "token_budget"
    }

    fn description(&self) -> &str {
        "Manage context within a token budget"
    }
}

/// Strategy that uses LLM to summarize old messages
///
/// This is an async strategy that uses the LLM to create summaries
/// of older conversation segments. It requires an LLM provider.
pub struct SummarizeStrategy {
    /// LLM provider for summarization
    provider: Arc<dyn LlmProvider>,
    /// Maximum messages to include in a single summary
    summary_batch_size: usize,
    /// Maximum tokens for the summary
    max_summary_tokens: usize,
}

impl SummarizeStrategy {
    /// Create a new summarize strategy
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            provider,
            summary_batch_size: 10,
            max_summary_tokens: 500,
        }
    }

    /// Create a summary of a batch of messages
    pub async fn summarize_batch(&self, messages: &[SessionMessage]) -> Option<String> {
        if messages.is_empty() {
            return None;
        }

        let content = messages
            .iter()
            .map(|m| format!("[{}]: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Summarize the following conversation in a concise way, preserving key information, decisions, and context. Keep it under {} tokens.\n\nConversation:\n{}",
            self.max_summary_tokens, content
        );

        // Note: This is a placeholder - actual implementation would call the LLM
        // and handle the response properly
        let _ = prompt; // Suppress unused warning
        None
    }

    fn estimate_tokens(messages: &[SessionMessage]) -> usize {
        messages.iter().map(|m| m.content.len() / 3).sum()
    }
}

impl HistoryStrategy for SummarizeStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        _current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        // For now, fall back to truncation
        // Full implementation would need async support
        let truncate = TruncateStrategy::default();
        truncate.process(history, _current_input, config)
    }

    fn name(&self) -> &str {
        "summarize"
    }

    fn description(&self) -> &str {
        "Use LLM to summarize old messages"
    }
}

/// Strategy that filters messages based on relevance to current input
///
/// This strategy attempts to identify messages that are semantically
/// relevant to the current query, and prioritizes those.
pub struct RelevanceFilterStrategy {
    /// Minimum relevance score (0.0-1.0) to include a message
    min_relevance: f32,
    /// Keywords to boost relevance
    boost_keywords: Vec<String>,
}

impl RelevanceFilterStrategy {
    /// Create a new relevance filter strategy
    pub fn new(min_relevance: f32) -> Self {
        Self {
            min_relevance,
            boost_keywords: Vec::new(),
        }
    }

    /// Add keywords that boost relevance
    pub fn with_boost_keywords(mut self, keywords: Vec<String>) -> Self {
        self.boost_keywords = keywords;
        self
    }

    /// Calculate relevance score between a message and current input
    fn calculate_relevance(&self, message: &str, current_input: &str) -> f32 {
        let msg_words: std::collections::HashSet<String> = message
            .split_whitespace()
            .map(|w| {
                w.to_lowercase()
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string()
            })
            .filter(|w| w.len() > 2)
            .collect();

        let input_words: std::collections::HashSet<String> = current_input
            .split_whitespace()
            .map(|w| {
                w.to_lowercase()
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string()
            })
            .filter(|w| w.len() > 2)
            .collect();

        if msg_words.is_empty() || input_words.is_empty() {
            return 0.0;
        }

        // Jaccard similarity
        let intersection = msg_words.intersection(&input_words).count();
        let union = msg_words.union(&input_words).count();

        let base_score = if union > 0 {
            intersection as f32 / union as f32
        } else {
            0.0
        };

        // Boost if any boost keywords are present
        let boost = if self
            .boost_keywords
            .iter()
            .any(|kw| message.to_lowercase().contains(&kw.to_lowercase()))
        {
            0.2
        } else {
            0.0
        };

        (base_score + boost).min(1.0)
    }
}

impl Default for RelevanceFilterStrategy {
    fn default() -> Self {
        Self {
            min_relevance: 0.1,
            boost_keywords: Vec::new(),
        }
    }
}

impl HistoryStrategy for RelevanceFilterStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        let max = config.max_messages;
        let recent_keep = config.recent_keep;

        // Always keep the most recent N messages
        let total = history.len();
        let protected_start = total.saturating_sub(recent_keep);

        let mut result = Vec::with_capacity(max);
        let mut filtered_count = 0;

        for (i, msg) in history.into_iter().enumerate() {
            if i >= protected_start {
                // Always include recent messages
                result.push(msg);
            } else {
                // Check relevance
                let relevance = self.calculate_relevance(&msg.content, current_input);
                if relevance >= self.min_relevance {
                    result.push(msg);
                } else {
                    filtered_count += 1;
                }
            }
        }

        // If we still have room, add more from the middle
        // (prioritizing more recent messages)

        let estimated_tokens = result.iter().map(|m| m.content.len() / 3).sum();

        ProcessedHistory {
            messages: result,
            estimated_tokens,
            filtered_count,
            summarized_count: 0,
        }
    }

    fn name(&self) -> &str {
        "relevance"
    }

    fn description(&self) -> &str {
        "Filter messages based on relevance to current input"
    }
}

/// Combined strategy that applies multiple strategies in sequence
///
/// This allows for sophisticated processing pipelines, e.g.:
/// 1. Filter by relevance
/// 2. Apply token budget
pub struct CombinedStrategy {
    strategies: Vec<Box<dyn HistoryStrategy>>,
}

impl CombinedStrategy {
    /// Create a new combined strategy
    pub fn new(strategies: Vec<Box<dyn HistoryStrategy>>) -> Self {
        Self { strategies }
    }
}

impl HistoryStrategy for CombinedStrategy {
    fn process(
        &self,
        history: Vec<SessionMessage>,
        current_input: &str,
        config: &HistoryConfig,
    ) -> ProcessedHistory {
        let mut current_history = history;
        let mut total_filtered = 0;
        let mut total_summarized = 0;
        let mut final_tokens = 0;

        for strategy in &self.strategies {
            let result = strategy.process(current_history, current_input, config);
            current_history = result.messages;
            total_filtered += result.filtered_count;
            total_summarized += result.summarized_count;
            final_tokens = result.estimated_tokens;
        }

        ProcessedHistory {
            messages: current_history,
            estimated_tokens: final_tokens,
            filtered_count: total_filtered,
            summarized_count: total_summarized,
        }
    }

    fn name(&self) -> &str {
        "combined"
    }

    fn description(&self) -> &str {
        "Apply multiple strategies in sequence"
    }
}

/// Factory for creating history strategies
pub struct StrategyFactory {
    config: HistoryConfig,
}

impl StrategyFactory {
    /// Create a new strategy factory
    pub fn new(config: HistoryConfig) -> Self {
        Self { config }
    }

    /// Create the default strategy based on configuration
    pub fn create_default(&self) -> Box<dyn HistoryStrategy> {
        Box::new(TruncateStrategy {
            recent_keep: self.config.recent_keep,
            truncate_length: self.config.truncate_length,
        })
    }

    /// Create a strategy by name
    pub fn create(&self, name: &str) -> Option<Box<dyn HistoryStrategy>> {
        match name {
            "direct" => Some(Box::new(DirectInjectStrategy)),
            "truncate" => Some(Box::new(TruncateStrategy {
                recent_keep: self.config.recent_keep,
                truncate_length: self.config.truncate_length,
            })),
            "token_budget" => Some(Box::new(TokenBudgetStrategy::new(
                self.config.token_budget,
                self.config.recent_keep,
            ))),
            "relevance" => Some(Box::new(RelevanceFilterStrategy::default())),
            _ => None,
        }
    }

    /// Create a combined strategy with relevance filtering and token budget
    pub fn create_smart(&self) -> Box<dyn HistoryStrategy> {
        Box::new(CombinedStrategy::new(vec![
            Box::new(RelevanceFilterStrategy::default()),
            Box::new(TokenBudgetStrategy::new(
                self.config.token_budget,
                self.config.recent_keep,
            )),
        ]))
    }
}

/// Truncate text to `max_chars` for context trimming.
///
/// Cuts at a safe UTF-8 char boundary and appends "...".
fn truncate_content(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut end = max_chars;
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_message(role: &str, content: &str) -> SessionMessage {
        SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            tools_used: None,
        }
    }

    #[test]
    fn test_direct_inject_strategy() {
        let strategy = DirectInjectStrategy;
        let config = HistoryConfig::default();
        let history = vec![
            make_message("user", "Hello"),
            make_message("assistant", "Hi there!"),
        ];

        let result = strategy.process(history, "test", &config);
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.filtered_count, 0);
    }

    #[test]
    fn test_truncate_strategy() {
        let strategy = TruncateStrategy {
            recent_keep: 1,
            truncate_length: 10,
        };
        let config = HistoryConfig::default();

        let history = vec![
            make_message(
                "user",
                "This is a very long message that should be truncated",
            ),
            make_message("assistant", "This should be kept in full"),
        ];

        let result = strategy.process(history, "test", &config);
        assert_eq!(result.messages.len(), 2);
        assert!(result.messages[0].content.ends_with("..."));
        assert_eq!(result.messages[1].content, "This should be kept in full");
    }

    #[test]
    fn test_token_budget_strategy() {
        let strategy = TokenBudgetStrategy {
            token_budget: 20, // Very small budget
            protected_recent: 1,
        };
        let config = HistoryConfig::default();

        let history = vec![
            make_message("user", "First message that is quite long"),
            make_message("assistant", "Second message that is also quite long"),
            make_message("user", "Short"), // This one should be protected
        ];

        let result = strategy.process(history, "test", &config);
        // At least the protected message should be included
        assert!(!result.messages.is_empty());
        assert!(result.filtered_count > 0 || result.estimated_tokens <= 20);
    }

    #[test]
    fn test_relevance_filter_strategy() {
        let strategy = RelevanceFilterStrategy {
            min_relevance: 0.1,
            boost_keywords: vec!["important".to_string()],
        };
        let config = HistoryConfig {
            recent_keep: 1,
            ..Default::default()
        };

        let history = vec![
            make_message("user", "Tell me about apples"),
            make_message("assistant", "Apples are fruits"),
            make_message("user", "What about bananas?"), // This one should be filtered
            make_message("assistant", "Bananas are also fruits"),
            make_message("user", "Do you like apples?"), // Recent, always included
        ];

        let result = strategy.process(history, "apples", &config);
        assert!(!result.messages.is_empty());
    }

    #[test]
    fn test_relevance_calculation() {
        let strategy = RelevanceFilterStrategy::default();

        // Same words -> high relevance
        let score = strategy.calculate_relevance("hello world", "hello world");
        assert!(score > 0.5);

        // No common words -> low relevance
        let score = strategy.calculate_relevance("foo bar", "baz qux");
        assert!(score < 0.2);

        // Some overlap -> medium relevance
        let score = strategy.calculate_relevance("hello foo", "hello bar");
        assert!(score > 0.0);
    }
}
