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
//! let context = AgentContext::persistent(event_store, compression_tx);
//!
//! // Subagent without persistence
//! let context = AgentContext::Stateless;
//!
//! // Use through enum methods
//! context.save_event(event).await?;
//! ```

use std::sync::Arc;
use tokio::sync::mpsc;

use crate::error::AgentError;
use gasket_storage::EventStore;
use gasket_types::SessionKey;
use gasket_types::{Session, SessionEvent, SummaryType};

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
#[derive(Debug, Clone)]
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
#[derive(Clone)]
pub struct PersistentContext {
    /// Event store for persisting events
    pub event_store: Arc<EventStore>,

    /// Compression task sender for background summarization
    pub compression_tx: mpsc::Sender<CompressionTask>,
}

impl std::fmt::Debug for PersistentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentContext")
            .field("event_store", &"EventStore { .. }")
            .field("compression_tx", &"mpsc::Sender<CompressionTask>")
            .finish()
    }
}

impl AgentContext {
    /// Create a persistent context with event store and compression channel.
    ///
    /// This is the main constructor for main agents that need persistence.
    ///
    /// # Arguments
    ///
    /// * `event_store` - Event store for persisting session events
    /// * `compression_tx` - Channel sender for background compression tasks
    ///
    /// # Example
    ///
    /// ```ignore
    /// let event_store = Arc::new(EventStore::new(pool));
    /// let (tx, _rx) = mpsc::channel(64);
    /// let context = AgentContext::persistent(event_store, tx);
    /// assert!(context.is_persistent());
    /// ```
    pub fn persistent(
        event_store: Arc<EventStore>,
        compression_tx: mpsc::Sender<CompressionTask>,
    ) -> Self {
        Self::Persistent(PersistentContext {
            event_store,
            compression_tx,
        })
    }

    /// Check if this context has persistence enabled.
    ///
    /// Returns `true` for `Persistent` variant, `false` for `Stateless`.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }

    /// Load a session for the given key.
    ///
    /// For `Persistent` context, loads events from EventStore and reconstructs session state.
    /// For `Stateless` context, creates a new in-memory session.
    pub async fn load_session(&self, key: &SessionKey) -> Session {
        match self {
            Self::Persistent(ctx) => {
                let events = ctx
                    .event_store
                    .get_branch_history(&key.to_string(), "main")
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("Failed to load session history for '{}': {}", key, e);
                        Vec::new()
                    });

                let mut session = Session::new(key.to_string());
                session.update_from_events(&events);
                session
            }
            Self::Stateless => Session::new(key.to_string()),
        }
    }

    /// Save an event to the session.
    ///
    /// For `Persistent` context, persists the event to the EventStore.
    /// For `Stateless` context, this is a no-op.
    ///
    /// # Errors
    ///
    /// Returns an error if the event cannot be persisted to the database.
    pub async fn save_event(&self, event: SessionEvent) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store
                    .append_event(&event)
                    .await
                    .map_err(|e| AgentError::Other(format!("Failed to persist event: {}", e)))?;
                Ok(())
            }
            Self::Stateless => Ok(()),
        }
    }

    /// Get history for a session.
    ///
    /// For `Persistent` context, retrieves events from the EventStore.
    /// For `Stateless` context, returns an empty vector.
    ///
    /// # Arguments
    ///
    /// * `key` - Session key to retrieve history for
    /// * `branch` - Optional branch name (defaults to "main" if None)
    ///
    /// # Returns
    ///
    /// A vector of session events in chronological order (oldest first).
    pub async fn get_history(&self, key: &str, branch: Option<&str>) -> Vec<SessionEvent> {
        match self {
            Self::Persistent(ctx) => ctx
                .event_store
                .get_branch_history(key, branch.unwrap_or("main"))
                .await
                .unwrap_or_default(),
            Self::Stateless => vec![],
        }
    }

    /// Trigger background compression for a session.
    ///
    /// Sends a compression task to the background compression actor.
    /// For `Stateless` context, this is a no-op.
    ///
    /// # Arguments
    ///
    /// * `task` - The compression task to process
    ///
    /// # Errors
    ///
    /// Returns an error if the task cannot be sent to the compression actor.
    pub async fn trigger_compression(&self, task: CompressionTask) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.compression_tx.send(task).await.map_err(|e| {
                    AgentError::Other(format!("Failed to send compression task: {}", e))
                })?;
                Ok(())
            }
            Self::Stateless => Ok(()),
        }
    }

    /// Recall relevant historical messages based on recency and content heuristics.
    ///
    /// Returns the top-K most relevant messages from the event store using
    /// a scoring function that combines recency and content length.
    /// For `Stateless` context, returns an empty vector.
    ///
    /// # Arguments
    ///
    /// * `key` - Session key to retrieve history for
    /// * `_query_embedding` - Query embedding (currently unused, reserved for future semantic search)
    /// * `top_k` - Maximum number of messages to return
    ///
    /// # Returns
    ///
    /// A vector of message contents, sorted by relevance score (highest first).
    pub async fn recall_history(
        &self,
        key: &str,
        _query_embedding: &[f32],
        top_k: usize,
    ) -> anyhow::Result<Vec<String>> {
        match self {
            Self::Persistent(ctx) => {
                let events = ctx
                    .event_store
                    .get_branch_history(key, "main")
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("Failed to load events for recall: {}", e);
                        Vec::new()
                    });

                if events.is_empty() {
                    return Ok(Vec::new());
                }

                // Score by recency (more recent = higher) and content length
                let mut scored: Vec<(f32, String)> = events
                    .iter()
                    .enumerate()
                    .map(|(idx, event)| {
                        let recency = idx as f32 / events.len() as f32;
                        let length = (event.content.len() as f32).ln() / 10.0;
                        (recency + length, event.content.clone())
                    })
                    .collect();

                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

                Ok(scored.into_iter().take(top_k).map(|(_, c)| c).collect())
            }
            Self::Stateless => Ok(Vec::new()),
        }
    }

    /// Clear session data from the event store.
    ///
    /// For `Persistent` context, clears all events and session data from the EventStore.
    /// For `Stateless` context, this is a no-op.
    ///
    /// # Arguments
    ///
    /// * `session_key` - The session key to clear
    ///
    /// # Errors
    ///
    /// Returns an error if the session cannot be cleared from the database.
    pub async fn clear_session(&self, session_key: &str) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store
                    .clear_session(session_key)
                    .await
                    .map_err(|e| AgentError::Other(format!("Failed to clear session: {}", e)))?;
                Ok(())
            }
            Self::Stateless => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gasket_storage::SqlitePool;
    use gasket_types::{EventMetadata, EventType};
    use sqlx::sqlite::SqlitePoolOptions;

    /// Setup an in-memory SQLite database for testing.
    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE sessions_v2 (
                key TEXT PRIMARY KEY,
                current_branch TEXT NOT NULL DEFAULT 'main',
                branches TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_consolidated_event TEXT,
                total_events INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE session_events (
                id TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                branch TEXT DEFAULT 'main',
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                event_data TEXT,
                extra TEXT DEFAULT '{}',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

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

    // ============================================================
    // Integration tests with real EventStore
    // ============================================================

    #[tokio::test]
    async fn test_persistent_context_creation() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);
        assert!(context.is_persistent());
    }

    #[tokio::test]
    async fn test_persistent_context_save_event() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);

        // Save event
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        let result = context.save_event(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_persistent_context_get_history() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);

        // Save multiple events
        let e1 = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        context.save_event(e1.clone()).await.unwrap();

        let e2 = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::AssistantMessage,
            content: "Hi there!".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        context.save_event(e2.clone()).await.unwrap();

        // Retrieve history
        let history = context.get_history("test:session", None).await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi there!");
    }

    #[tokio::test]
    async fn test_persistent_context_get_history_with_branch() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);

        // Save event to main branch
        let main_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Main branch message".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        context.save_event(main_event).await.unwrap();

        // Save event to feature branch
        let feature_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Feature branch message".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("feature".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        context.save_event(feature_event).await.unwrap();

        // Query main branch
        let main_history = context.get_history("test:session", Some("main")).await;
        assert_eq!(main_history.len(), 1);
        assert_eq!(main_history[0].content, "Main branch message");

        // Query feature branch
        let feature_history = context.get_history("test:session", Some("feature")).await;
        assert_eq!(feature_history.len(), 1);
        assert_eq!(feature_history[0].content, "Feature branch message");
    }

    #[tokio::test]
    async fn test_persistent_context_trigger_compression() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, mut rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);

        // Create and send compression task
        let task = CompressionTask {
            session_key: "test:session".to_string(),
            branch: "main".to_string(),
            evicted_events: vec![uuid::Uuid::now_v7()],
            compression_type: SummaryType::Compression { token_budget: 1000 },
            retry_count: 0,
        };

        let result = context.trigger_compression(task.clone()).await;
        assert!(result.is_ok());

        // Verify task was received
        let received = rx.recv().await;
        assert!(received.is_some());
        let received_task = received.unwrap();
        assert_eq!(received_task.session_key, "test:session");
        assert_eq!(received_task.branch, "main");
    }

    #[tokio::test]
    async fn test_persistent_context_flow() {
        // Setup in-memory database
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);
        assert!(context.is_persistent());

        // Save event
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        context.save_event(event).await.unwrap();

        // Load history
        let history = context.get_history("test:session", None).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Hello");
    }

    #[tokio::test]
    async fn test_persistent_context_clear_session() {
        let pool = setup_test_db().await;
        let event_store = Arc::new(EventStore::new(pool));
        let (tx, _rx) = mpsc::channel(1);

        let context = AgentContext::persistent(event_store, tx);
        assert!(context.is_persistent());

        // Save event
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        context.save_event(event).await.unwrap();

        // Verify history exists
        let history = context.get_history("test:session", None).await;
        assert_eq!(history.len(), 1);

        // Clear session
        let result = context.clear_session("test:session").await;
        assert!(result.is_ok());

        // Verify history is cleared
        let history = context.get_history("test:session", None).await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_stateless_context_clear_session() {
        let context = AgentContext::Stateless;

        // Clear session should be a no-op for stateless context
        let result = context.clear_session("test:session").await;
        assert!(result.is_ok());
    }
}
