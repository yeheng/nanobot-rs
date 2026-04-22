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

    /// Event metadata
    pub metadata: EventMetadata,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Monotonically increasing sequence number for incremental sync and checkpointing.
    pub sequence: i64,
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

    /// Map this event type to an LLM conversation role string.
    ///
    /// Single source of truth for `EventType → role` mapping.
    /// When adding new event types, the exhaustive match here
    /// ensures no silent fallback to "system".
    pub fn role_str(&self) -> &'static str {
        match self {
            EventType::UserMessage => "user",
            EventType::AssistantMessage => "assistant",
            EventType::ToolCall { .. } => "tool",
            EventType::ToolResult { .. } => "tool",
            EventType::Summary { .. } => "system",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::UserMessage => write!(f, "User"),
            EventType::AssistantMessage => write!(f, "Assistant"),
            EventType::ToolCall { tool_name, .. } => write!(f, "Tool({})", tool_name),
            EventType::ToolResult { tool_name, .. } => write!(f, "ToolResult({})", tool_name),
            EventType::Summary { summary_type, .. } => write!(f, "Summary({:?})", summary_type),
        }
    }
}

impl SessionEvent {
    /// Return pre-computed content token count from DB.
    /// Returns 0 if not yet computed (e.g., events created in-memory before persistence).
    pub fn token_len_cached(&self) -> usize {
        self.metadata.content_token_len
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
    /// List of tools used
    #[serde(default)]
    pub tools_used: Vec<String>,

    /// Token statistics (LLM API input/output tokens)
    pub token_usage: Option<crate::token_tracker::TokenUsage>,

    /// Pre-computed content token count via tiktoken BPE encoding.
    /// Calculated once at write time in `append_event` to avoid
    /// re-computation on every read in `process_history`.
    #[serde(default)]
    pub content_token_len: usize,

    /// Extension fields
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
        if events.is_empty() {
            return;
        }

        self.metadata.created_at = events[0].created_at;

        for event in events {
            self.metadata.total_events += 1;
            if let Some(ref usage) = event.metadata.token_usage {
                self.metadata.total_tokens += (usage.input_tokens + usage.output_tokens) as u64;
            }
            if event.event_type.is_summary() {
                self.metadata.last_consolidated_event = Some(event.id);
            }
        }

        self.metadata.updated_at = events.last().unwrap().created_at;
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
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
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
