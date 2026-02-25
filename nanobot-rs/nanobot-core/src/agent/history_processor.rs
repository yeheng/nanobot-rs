//! History context processor for managing conversation history
//!
//! Provides a simple token-budget-aware history truncation function.
//! Keeps recent messages verbatim, truncates older messages to fit budget.

use crate::session::SessionMessage;

/// Configuration for history processing
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Maximum number of messages to include
    pub max_messages: usize,
    /// Token budget for history (0 = unlimited)
    pub token_budget: usize,
    /// Number of recent messages to always keep
    pub recent_keep: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_messages: 50,
            token_budget: 8000, // ~8k tokens for context window
            recent_keep: 10,
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
}

/// Process history with token budget awareness.
///
/// Simple algorithm:
/// 1. Take up to `max_messages` most recent messages
/// 2. Always keep the last `recent_keep` messages verbatim
/// 3. For older messages, include them only if they fit within the token budget
///
/// Token estimation: ~3 characters per token (rough middle ground for mixed content)
pub fn process_history(history: Vec<SessionMessage>, config: &HistoryConfig) -> ProcessedHistory {
    if history.is_empty() {
        return ProcessedHistory {
            messages: Vec::new(),
            estimated_tokens: 0,
            filtered_count: 0,
        };
    }

    // Take up to max_messages from the end
    let total = history.len().min(config.max_messages);
    let start_idx = history.len().saturating_sub(total);
    let messages: Vec<SessionMessage> = history.into_iter().skip(start_idx).collect();

    // Split into protected (recent) and older messages
    let protected_start = messages.len().saturating_sub(config.recent_keep);
    let older: Vec<SessionMessage> = messages.iter().take(protected_start).cloned().collect();
    let protected: Vec<SessionMessage> = messages.iter().skip(protected_start).cloned().collect();

    // Calculate tokens for protected messages (always included)
    let protected_tokens: usize = protected.iter().map(|m| count_tokens(&m.content)).sum();
    let mut current_tokens = protected_tokens;
    let mut included_older: Vec<SessionMessage> = Vec::new();

    // Add older messages from most recent to oldest, respecting budget
    for msg in older.into_iter().rev() {
        let msg_tokens = count_tokens(&msg.content);

        if config.token_budget == 0 || current_tokens + msg_tokens <= config.token_budget {
            current_tokens += msg_tokens;
            included_older.push(msg);
        }
        // Skip if budget exceeded
    }

    // Reorder: older messages first, then protected
    included_older.reverse();
    let mut result = included_older;
    result.extend(protected);

    let filtered_count = total - result.len();

    ProcessedHistory {
        messages: result,
        estimated_tokens: current_tokens,
        filtered_count,
    }
}

/// Estimate token count for text.
/// Uses ~3 characters per token as a rough estimate.
fn count_tokens(text: &str) -> usize {
    text.len() / 3.max(1)
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
    fn test_empty_history() {
        let config = HistoryConfig::default();
        let result = process_history(vec![], &config);
        assert!(result.messages.is_empty());
        assert_eq!(result.filtered_count, 0);
    }

    #[test]
    fn test_small_history_included_full() {
        let config = HistoryConfig {
            max_messages: 50,
            token_budget: 10000,
            recent_keep: 5,
        };

        let history = vec![
            make_message("user", "Hello"),
            make_message("assistant", "Hi there!"),
        ];

        let result = process_history(history, &config);
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.filtered_count, 0);
    }

    #[test]
    fn test_token_budget_filtering() {
        let config = HistoryConfig {
            max_messages: 50,
            token_budget: 10, // Very small budget
            recent_keep: 1,   // Keep last message
        };

        let history = vec![
            make_message("user", "First message that is quite long"),
            make_message("assistant", "Second message that is also quite long"),
            make_message("user", "Short"), // This one should be protected
        ];

        let result = process_history(history, &config);
        // At least the protected message should be included
        assert!(!result.messages.is_empty());
        // Last message should be "Short"
        assert_eq!(result.messages.last().unwrap().content, "Short");
        assert!(result.filtered_count > 0);
    }

    #[test]
    fn test_max_messages_limit() {
        let config = HistoryConfig {
            max_messages: 3,
            token_budget: 0, // Unlimited
            recent_keep: 1,
        };

        let history: Vec<SessionMessage> = (0..10)
            .map(|i| make_message("user", &format!("Message {}", i)))
            .collect();

        let result = process_history(history, &config);
        assert_eq!(result.messages.len(), 3);
        // Should have messages 7, 8, 9
        assert_eq!(result.messages[0].content, "Message 7");
        assert_eq!(result.messages[2].content, "Message 9");
    }

    #[test]
    fn test_recent_keep_always_included() {
        let config = HistoryConfig {
            max_messages: 10,
            token_budget: 1, // Very restrictive
            recent_keep: 3,  // Keep last 3
        };

        let history: Vec<SessionMessage> = (0..5)
            .map(|i| make_message("user", &format!("Message {} with some extra content", i)))
            .collect();

        let result = process_history(history, &config);
        // Should have at least the 3 protected messages
        assert!(result.messages.len() >= 3);
        // Last 3 should be messages 2, 3, 4
        let contents: Vec<&str> = result.messages.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.iter().any(|c| c.starts_with("Message 2")));
        assert!(contents.iter().any(|c| c.starts_with("Message 3")));
        assert!(contents.iter().any(|c| c.starts_with("Message 4")));
    }
}
