//! History context processor for managing conversation history
//!
//! Provides a simple token-budget-aware history truncation function.
//! Keeps recent events verbatim, truncates older events to fit budget.
//! Uses tiktoken-rs for accurate BPE token counting.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;
use tracing::warn;

use gasket_types::SessionEvent;

/// Configuration for history processing
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Maximum number of events to include
    pub max_events: usize,
    /// Token budget for history (0 = unlimited)
    pub token_budget: usize,
    /// Number of recent events to always keep
    pub recent_keep: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_events: 50,
            token_budget: 8000, // ~8k tokens for context window
            recent_keep: 10,
        }
    }
}

/// Result of processing history
#[derive(Debug, Clone)]
pub struct ProcessedHistory {
    /// The processed events (retained for context)
    pub events: Vec<SessionEvent>,
    /// Estimated token count
    pub estimated_tokens: usize,
    /// Number of events that were filtered out
    pub filtered_count: usize,
    /// Events that were evicted (exceeded budget)
    pub evicted: Vec<SessionEvent>,
}

/// Process history with token budget awareness.
///
/// Simple algorithm:
/// 1. Take up to `max_events` most recent events
/// 2. Always keep the last `recent_keep` events verbatim
/// 3. For older events, include them only if they fit within the token budget
///
/// Token counting uses tiktoken-rs (cl100k_base BPE encoding) for accuracy.
pub fn process_history(history: Vec<SessionEvent>, config: &HistoryConfig) -> ProcessedHistory {
    if history.is_empty() {
        return ProcessedHistory {
            events: Vec::new(),
            estimated_tokens: 0,
            filtered_count: 0,
            evicted: Vec::new(),
        };
    }

    // Take up to max_events from the end
    let total = history.len().min(config.max_events);
    let start_idx = history.len().saturating_sub(total);
    let mut events: Vec<SessionEvent> = history.into_iter().skip(start_idx).collect();

    // Split into protected (recent) and older events using zero-copy split_off
    let protected_start = events.len().saturating_sub(config.recent_keep);
    let protected = events.split_off(protected_start);
    let older = events; // Remaining events are the older ones

    // Calculate tokens for protected events (always included)
    let protected_tokens: usize = protected.iter().map(|e| count_tokens(&e.content)).sum();
    let mut current_tokens = protected_tokens;
    let mut included_older: Vec<SessionEvent> = Vec::new();
    let mut evicted: Vec<SessionEvent> = Vec::new();

    // Add older events from most recent to oldest, respecting budget
    // Use into_iter() to take ownership instead of cloning
    let mut budget_exceeded = false;
    for event in older.into_iter().rev() {
        let event_tokens = count_tokens(&event.content);

        if !budget_exceeded
            && (config.token_budget == 0 || current_tokens + event_tokens <= config.token_budget)
        {
            current_tokens += event_tokens;
            included_older.push(event);
        } else {
            // Budget exceeded - this event and all older events are evicted
            budget_exceeded = true;
            evicted.push(event);
        }
    }

    // Reorder: older events first, then protected
    included_older.reverse();
    // Evicted events are in reverse order (newest first), reorder to chronological
    evicted.reverse();

    let mut result = included_older;
    result.extend(protected);

    let filtered_count = total - result.len();

    ProcessedHistory {
        events: result,
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
    use chrono::Utc;
    use gasket_types::{EventMetadata, EventType};

    fn make_event(event_type: EventType, content: &str) -> SessionEvent {
        SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test".into(),
            parent_id: None,
            event_type,
            content: content.to_string().into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_empty_history() {
        let config = HistoryConfig::default();
        let result = process_history(vec![], &config);
        assert!(result.events.is_empty());
        assert_eq!(result.filtered_count, 0);
    }

    #[test]
    fn test_small_history_included_full() {
        let config = HistoryConfig {
            max_events: 50,
            token_budget: 10000,
            recent_keep: 5,
        };

        let history = vec![
            make_event(EventType::UserMessage, "Hello"),
            make_event(EventType::AssistantMessage, "Hi there!"),
        ];

        let result = process_history(history, &config);
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.filtered_count, 0);
    }

    #[test]
    fn test_token_budget_filtering() {
        let config = HistoryConfig {
            max_events: 50,
            token_budget: 10, // Very small budget
            recent_keep: 1,   // Keep last event
        };

        let history = vec![
            make_event(EventType::UserMessage, "First message that is quite long"),
            make_event(
                EventType::AssistantMessage,
                "Second message that is also quite long",
            ),
            make_event(EventType::UserMessage, "Short"), // This one should be protected
        ];

        let result = process_history(history, &config);
        // At least the protected event should be included
        assert!(!result.events.is_empty());
        // Last event should be "Short"
        assert_eq!(result.events.last().unwrap().content, "Short");
        assert!(result.filtered_count > 0);
    }

    #[test]
    fn test_max_events_limit() {
        let config = HistoryConfig {
            max_events: 3,
            token_budget: 0, // Unlimited
            recent_keep: 1,
        };

        let history: Vec<SessionEvent> = (0..10)
            .map(|i| make_event(EventType::UserMessage, &format!("Message {}", i)))
            .collect();

        let result = process_history(history, &config);
        assert_eq!(result.events.len(), 3);
        // Should have events 7, 8, 9
        assert_eq!(result.events[0].content, "Message 7");
        assert_eq!(result.events[2].content, "Message 9");
    }

    #[test]
    fn test_recent_keep_always_included() {
        let config = HistoryConfig {
            max_events: 10,
            token_budget: 1, // Very restrictive
            recent_keep: 3,  // Keep last 3
        };

        let history: Vec<SessionEvent> = (0..5)
            .map(|i| {
                make_event(
                    EventType::UserMessage,
                    &format!("Message {} with some extra content", i),
                )
            })
            .collect();

        let result = process_history(history, &config);
        // Should have at least the 3 protected events
        assert!(result.events.len() >= 3);
        // Last 3 should be messages 2, 3, 4
        let contents: Vec<&str> = result.events.iter().map(|e| e.content.as_str()).collect();
        assert!(contents.iter().any(|c| c.starts_with("Message 2")));
        assert!(contents.iter().any(|c| c.starts_with("Message 3")));
        assert!(contents.iter().any(|c| c.starts_with("Message 4")));
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
