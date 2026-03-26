//! Agent context using Enum instead of trait for zero runtime dispatch.
//!
//! This module provides an enum-based abstraction for agent state management,
//! eliminating `Arc<dyn AgentContext>` trait object overhead.
//!
//! # Design
//!
//! - `AgentContext` enum has `Persistent` and `Stateless` variants
//! - `PersistentContext` holds references to services (main agents)
//! - `Stateless` variant provides no-op implementations (subagents)
//!
//! # Benefits
//!
//! - Zero runtime dispatch overhead (enum dispatch vs trait object vtable)
//! - Better cache locality (enum variants are inline)
//! - Compile-time exhaustiveness checking
//!
//! # Example
//!
//! ```ignore
//! // Main agent with persistence
//! let context = AgentContext::Persistent(persistent_ctx);
//!
//! // Subagent without persistence
//! let context = AgentContext::Stateless;
//!
//! // Use through enum methods
//! context.save_event(event).await?;
//! ```

use std::sync::Arc;
use tokio::sync::mpsc;

use crate::bus::events::SessionKey;
use crate::error::AgentError;
use gasket_types::{Session, SessionEvent, SummaryType};

// Forward declarations for types we'll create later
/// Event store for persisting session events.
/// Will be implemented in Task 5.
#[derive(Debug)]
pub struct EventStore;

/// History retriever for querying past events.
/// Will be implemented in Task 7.
#[derive(Debug)]
pub struct HistoryRetriever;

/// Embedding service for semantic search.
/// Will be implemented in a future task.
#[derive(Debug)]
pub struct EmbeddingService;

/// Session manager for session lifecycle.
/// Will be connected to the new event-sourced SessionManager.
#[derive(Debug)]
pub struct SessionManager;

/// Compression task for background summarization.
#[derive(Debug, Clone)]
pub struct CompressionTask {
    /// Session key for the task
    pub session_key: String,
    /// Branch name
    pub branch: String,
    /// Events to be compressed
    pub evicted_events: Vec<uuid::Uuid>,
    /// Type of compression
    pub compression_type: SummaryType,
    /// Number of retry attempts
    pub retry_count: u32,
}

/// Agent context - using Enum instead of trait for zero runtime dispatch.
///
/// This enum provides two variants:
/// - `Persistent`: Full persistence support for main agents
/// - `Stateless`: No-op implementations for subagents
#[derive(Debug)]
pub enum AgentContext {
    /// Persistent context (main Agent)
    Persistent(PersistentContext),

    /// Stateless context (sub Agent)
    Stateless,
}

/// Persistent context data for main agents.
///
/// Holds references to all services needed for session persistence,
/// event storage, and context compression.
#[derive(Debug)]
pub struct PersistentContext {
    /// Session manager for session lifecycle
    pub session_manager: Arc<SessionManager>,

    /// Event store for persisting events
    pub event_store: Arc<EventStore>,

    /// History retriever for querying past events
    pub history_retriever: Arc<HistoryRetriever>,

    /// Embedding service for semantic search
    pub embedding_service: Arc<EmbeddingService>,

    /// Compression task sender for background summarization
    pub compression_tx: mpsc::Sender<CompressionTask>,
}

impl AgentContext {
    /// Check if this context has persistence enabled.
    ///
    /// Returns `true` for `Persistent` variant, `false` for `Stateless`.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }

    /// Load a session for the given key.
    ///
    /// For `Persistent` context, will load from storage (stub for now).
    /// For `Stateless` context, creates a new in-memory session.
    pub async fn load_session(&self, key: &SessionKey) -> Session {
        match self {
            Self::Persistent(_) => Session::new(key.to_string()),
            Self::Stateless => Session::new(key.to_string()),
        }
    }

    /// Save an event to the session.
    ///
    /// For `Persistent` context, will persist to storage (stub for now).
    /// For `Stateless` context, this is a no-op.
    pub async fn save_event(&self, _event: SessionEvent) -> Result<(), AgentError> {
        match self {
            Self::Persistent(_) => Ok(()), // Will be implemented with EventStore
            Self::Stateless => Ok(()),
        }
    }

    /// Get history for a session.
    ///
    /// For `Persistent` context, will query from storage (stub for now).
    /// For `Stateless` context, returns empty vector.
    pub async fn get_history(&self, key: &str, branch: Option<&str>) -> Vec<SessionEvent> {
        let _ = (key, branch);
        match self {
            Self::Persistent(_) => vec![], // Will be implemented with HistoryRetriever
            Self::Stateless => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::EventType;

    #[test]
    fn test_stateless_context_is_not_persistent() {
        let context = AgentContext::Stateless;
        assert!(!context.is_persistent());
    }

    #[tokio::test]
    async fn test_stateless_load_session() {
        let context = AgentContext::Stateless;
        let key = SessionKey::new(gasket_types::ChannelType::Cli, "test");
        let session = context.load_session(&key).await;
        assert_eq!(session.key, key.to_string());
    }

    #[tokio::test]
    async fn test_stateless_save_event() {
        let context = AgentContext::Stateless;
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "test".into(),
            embedding: None,
            metadata: Default::default(),
            created_at: chrono::Utc::now(),
        };
        let result = context.save_event(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stateless_get_history() {
        let context = AgentContext::Stateless;
        let history = context.get_history("test", None).await;
        assert!(history.is_empty());
    }

    #[test]
    fn test_compression_task_debug() {
        let task = CompressionTask {
            session_key: "cli:test".to_string(),
            branch: "main".to_string(),
            evicted_events: vec![uuid::Uuid::now_v7()],
            compression_type: SummaryType::Compression { token_budget: 1000 },
            retry_count: 0,
        };
        // Just verify Debug trait is implemented correctly
        let debug_str = format!("{:?}", task);
        assert!(debug_str.contains("CompressionTask"));
        assert!(debug_str.contains("cli:test"));
    }
}
