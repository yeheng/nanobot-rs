//! History context processor for managing conversation history
//!
//! Provides a simple token-budget-aware history truncation function.
//! Keeps recent messages verbatim, truncates older messages to fit budget.
//! Uses tiktoken-rs for accurate BPE token counting.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;
use tracing::warn;

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
    /// The processed messages (retained for context)
    pub messages: Vec<SessionMessage>,
    /// Estimated token count
    pub estimated_tokens: usize,
    /// Number of messages that were filtered out
    pub filtered_count: usize,
    /// Messages that were evicted (exceeded budget) - these should be summarized
    pub evicted: Vec<SessionMessage>,
}

/// Process history with token budget awareness.
///
/// Simple algorithm:
/// 1. Take up to `max_messages` most recent messages
/// 2. Always keep the last `recent_keep` messages verbatim
/// 3. For older messages, include them only if they fit within the token budget
///
/// Token counting uses tiktoken-rs (cl100k_base BPE encoding) for accuracy.
pub fn process_history(history: Vec<SessionMessage>, config: &HistoryConfig) -> ProcessedHistory {
    if history.is_empty() {
        return ProcessedHistory {
            messages: Vec::new(),
            estimated_tokens: 0,
            filtered_count: 0,
            evicted: Vec::new(),
        };
    }

    // Take up to max_messages from the end
    let total = history.len().min(config.max_messages);
    let start_idx = history.len().saturating_sub(total);
    let mut messages: Vec<SessionMessage> = history.into_iter().skip(start_idx).collect();

    // Split into protected (recent) and older messages using zero-copy split_off
    let protected_start = messages.len().saturating_sub(config.recent_keep);
    let protected = messages.split_off(protected_start);
    let older = messages; // Remaining messages are the older ones

    // Calculate tokens for protected messages (always included)
    let protected_tokens: usize = protected.iter().map(|m| count_tokens(&m.content)).sum();
    let mut current_tokens = protected_tokens;
    let mut included_older: Vec<SessionMessage> = Vec::new();
    let mut evicted: Vec<SessionMessage> = Vec::new();

    // Add older messages from most recent to oldest, respecting budget
    // Use into_iter() to take ownership instead of cloning
    let mut budget_exceeded = false;
    for msg in older.into_iter().rev() {
        let msg_tokens = count_tokens(&msg.content);

        if !budget_exceeded
            && (config.token_budget == 0 || current_tokens + msg_tokens <= config.token_budget)
        {
            current_tokens += msg_tokens;
            included_older.push(msg);
        } else {
            // Budget exceeded - this message and all older messages are evicted
            budget_exceeded = true;
            evicted.push(msg);
        }
    }

    // Reorder: older messages first, then protected
    included_older.reverse();
    // Evicted messages are in reverse order (newest first), reorder to chronological
    evicted.reverse();

    let mut result = included_older;
    result.extend(protected);

    let filtered_count = total - result.len();

    ProcessedHistory {
        messages: result,
        estimated_tokens: current_tokens,
        filtered_count,
        evicted,
    }
}

/// Global cached BPE encoder (cl100k_base, covers GPT-4/GPT-3.5).
static ENCODER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_encoder() -> Option<&'static CoreBPE> {
    ENCODER
        .get_or_init(|| match tiktoken_rs::cl100k_base() {
            Ok(enc) => Some(enc),
            Err(e) => {
                warn!(
                    "Failed to init tiktoken cl100k_base encoder: {}. Falling back to len/4.",
                    e
                );
                None
            }
        })
        .as_ref()
}

/// Count tokens using tiktoken-rs BPE encoding.
///
/// Falls back to `text.len() / 4` if the encoder fails to initialize.
pub fn count_tokens(text: &str) -> usize {
    match get_encoder() {
        Some(enc) => enc.encode_with_special_tokens(text).len(),
        None => text.len() / 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MessageRole;
    use chrono::Utc;

    fn make_message(role: MessageRole, content: &str) -> SessionMessage {
        SessionMessage {
            role,
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
            make_message(MessageRole::User, "Hello"),
            make_message(MessageRole::Assistant, "Hi there!"),
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
            make_message(MessageRole::User, "First message that is quite long"),
            make_message(
                MessageRole::Assistant,
                "Second message that is also quite long",
            ),
            make_message(MessageRole::User, "Short"), // This one should be protected
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
            .map(|i| make_message(MessageRole::User, &format!("Message {}", i)))
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
            .map(|i| {
                make_message(
                    MessageRole::User,
                    &format!("Message {} with some extra content", i),
                )
            })
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

    #[test]
    fn test_contiguous_eviction() {
        let config = HistoryConfig {
            max_messages: 50,
            token_budget: 15,
            recent_keep: 1,
        };

        // Middle message is large and exceeds budget.
        // Even though the first message is small, it must be evicted to maintain continuity.
        let history = vec![
            make_message(MessageRole::User, "Short 1"),
            make_message(
                MessageRole::User,
                "Very long message 2 that exceeds the remaining budget significantly..........",
            ),
            make_message(MessageRole::User, "Short 3"),
        ];

        let result = process_history(history, &config);

        // Only "Short 3" is included.
        // "Very long message 2..." and "Short 1" are evicted.
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].content, "Short 3");

        // Evicted order should be chronological
        assert_eq!(result.evicted.len(), 2);
        assert_eq!(result.evicted[0].content, "Short 1");
        assert!(result.evicted[1].content.starts_with("Very long"));
    }

    #[test]
    fn test_count_tokens_accuracy() {
        // "hello world" is 2 tokens in cl100k_base
        let tokens = count_tokens("hello world");
        assert!(
            tokens > 0,
            "count_tokens should return non-zero for non-empty text"
        );
        assert!(
            tokens < 10,
            "count_tokens should return reasonable count for short text"
        );

        // CJK text: each character is typically 1-2 tokens
        let cjk_tokens = count_tokens("你好世界");
        assert!(cjk_tokens > 0, "count_tokens should handle CJK text");

        // Empty string should be 0
        assert_eq!(count_tokens(""), 0);
    }
}
