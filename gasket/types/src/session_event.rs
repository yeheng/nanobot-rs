//! Session event types for event sourcing architecture.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Session event - immutable fact record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Event unique identifier (UUID v7 time-ordered)
    pub id: Uuid,

    /// Session this event belongs to
    pub session_key: String,

    /// Event type
    pub event_type: EventType,

    /// Message content
    pub content: String,

    /// Semantic vector (per-message embedding)
    pub embedding: Option<Vec<f32>>,

    /// Event metadata
    pub metadata: EventMetadata,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Event type enumeration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    /// User message
    UserMessage,

    /// Assistant reply
    AssistantMessage,

    /// Tool call
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// Tool result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
    },

    /// Summary event (compression generated)
    Summary {
        summary_type: SummaryType,
        covered_event_ids: Vec<Uuid>,
    },
}

impl EventType {
    /// Check if this is a summary type event.
    pub fn is_summary(&self) -> bool {
        matches!(self, EventType::Summary { .. })
    }
}

/// Summary type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummaryType {
    /// Time window summary
    TimeWindow { duration_hours: u32 },

    /// Topic summary
    Topic { topic: String },

    /// Compression summary (when exceeding token budget)
    Compression { token_budget: usize },
}

/// Event metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Branch name (None means main branch)
    pub branch: Option<String>,

    /// List of tools used
    #[serde(default)]
    pub tools_used: Vec<String>,

    /// Token statistics
    pub token_usage: Option<TokenUsage>,

    /// Extension fields
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Session - aggregate root for events.
#[derive(Debug, Clone)]
pub struct Session {
    /// Session identifier
    pub key: String,

    /// Session metadata
    pub metadata: SessionMetadata,
}

/// Session metadata.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,

    /// Last consolidation point (event ID)
    pub last_consolidated_event: Option<Uuid>,

    /// Total message count
    pub total_events: usize,

    /// Cumulative token usage
    pub total_tokens: u64,
}

impl Session {
    /// Create a new session.
    pub fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        let now = Utc::now();
        Self {
            key,
            metadata: SessionMetadata {
                created_at: now,
                updated_at: now,
                ..Default::default()
            },
        }
    }

    /// Create from a SessionKey.
    pub fn from_key(key: crate::SessionKey) -> Self {
        Self::new(key.to_string())
    }

    pub fn update_from_events(&mut self, events: &[SessionEvent]) {
        for event in events {
            self.metadata.total_events += 1;
            if let Some(ref usage) = event.metadata.token_usage {
                self.metadata.total_tokens += (usage.input_tokens + usage.output_tokens) as u64;
            }
        }
        self.metadata.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_serialization() {
        let event_type = EventType::UserMessage;
        let json = serde_json::to_string(&event_type).unwrap();
        assert!(json.contains("UserMessage"));
    }

    #[test]
    fn test_session_event_roundtrip() {
        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, "Hello");
    }

    #[test]
    fn test_session_new() {
        let session = Session::new("test:session");
        assert_eq!(session.key, "test:session");
    }
}
