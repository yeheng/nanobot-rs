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
//! let context = AgentContext::persistent(event_store);
//!
//! // Subagent without persistence
//! let context = AgentContext::Stateless;
//!
//! // Use through enum methods
//! context.save_event(event).await?;
//! ```

use std::sync::Arc;
use tracing::debug;

use crate::error::AgentError;
use gasket_storage::EventStore;
use gasket_types::SessionKey;
use gasket_types::{Session, SessionEvent};

use super::history::coordinator::HistoryCoordinator;

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
/// Holds references to the event store needed for session persistence,
/// and an optional text embedder for automatic embedding generation
/// on every saved event (decoupled from summarization/compaction).
#[derive(Clone)]
pub struct PersistentContext {
    /// Event store for persisting events
    pub event_store: Arc<EventStore>,
    /// SQLite store for saving embeddings (semantic recall index)
    pub sqlite_store: Arc<gasket_storage::SqliteStore>,
    /// Optional text embedder for automatic embedding generation.
    /// When present, every saved event gets an embedding for semantic recall,
    /// regardless of whether compaction/summarization is enabled.
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<gasket_storage::TextEmbedder>>,
    /// Optional HistoryCoordinator for unified history queries.
    /// Set after construction once all dependencies are available.
    pub coordinator: Option<Arc<HistoryCoordinator>>,
}

impl std::fmt::Debug for PersistentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentContext")
            .field("event_store", &"EventStore { .. }")
            .finish()
    }
}

impl PersistentContext {
    /// Set the HistoryCoordinator for this context.
    /// Called after construction once all dependencies are available.
    pub fn set_coordinator(&mut self, coordinator: Arc<HistoryCoordinator>) {
        self.coordinator = Some(coordinator);
    }
}

impl AgentContext {
    /// Create a persistent context with event store.
    ///
    /// This is the main constructor for main agents that need persistence.
    ///
    /// # Arguments
    ///
    /// * `event_store` - Event store for persisting session events
    ///
    /// # Example
    ///
    /// ```ignore
    /// let event_store = Arc::new(EventStore::new(pool));
    /// let context = AgentContext::persistent(event_store);
    /// assert!(context.is_persistent());
    /// ```
    pub fn persistent(
        event_store: Arc<EventStore>,
        sqlite_store: Arc<gasket_storage::SqliteStore>,
    ) -> Self {
        Self::Persistent(PersistentContext {
            event_store,
            sqlite_store,
            #[cfg(feature = "local-embedding")]
            embedder: None,
            coordinator: None,
        })
    }

    /// Check if this context has persistence enabled.
    ///
    /// Returns `true` for `Persistent` variant, `false` for `Stateless`.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }

    /// Load the summary and its sequence watermark for a session.
    ///
    /// Returns `(summary_text, covered_upto_sequence)`.
    /// For `Stateless` context or if no summary exists, returns `("", 0)`.
    pub async fn load_summary_with_watermark(&self, session_key: &str) -> (String, i64) {
        match self {
            Self::Persistent(ctx) => {
                match ctx.sqlite_store.load_session_summary(session_key).await {
                    Ok(Some((content, watermark))) => (content, watermark),
                    Ok(None) => (String::new(), 0),
                    Err(e) => {
                        debug!("Failed to load summary for {}: {}", session_key, e);
                        (String::new(), 0)
                    }
                }
            }
            Self::Stateless => (String::new(), 0),
        }
    }

    /// Load events after a sequence watermark for a session.
    ///
    /// Returns only events with `sequence > watermark`, i.e., events not yet
    /// covered by the summary. For `Stateless` context, returns empty vector.
    pub async fn get_events_after_watermark(
        &self,
        session_key: &str,
        watermark: i64,
    ) -> Vec<SessionEvent> {
        match self {
            Self::Persistent(ctx) => {
                if watermark == 0 {
                    // No summary exists — load all history
                    ctx.event_store
                        .get_branch_history(session_key, "main")
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to load history for '{}': {}", session_key, e);
                            Vec::new()
                        })
                } else {
                    ctx.event_store
                        .get_events_after_sequence(session_key, watermark)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                "Failed to load events after watermark for '{}': {}",
                                session_key,
                                e
                            );
                            Vec::new()
                        })
                }
            }
            Self::Stateless => vec![],
        }
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

                // Embedding generation is handled directly via IndexingService — no need to
                // generate inline on the hot path.

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
    /// # Note
    /// The `_query_embedding` parameter is currently unused but reserved for
    /// future semantic search support via `HistoryCoordinator`.
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
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT '',
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
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT '',
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                branch TEXT DEFAULT 'main',
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                token_len INTEGER NOT NULL DEFAULT 0,
                event_data TEXT,
                extra TEXT DEFAULT '{}',
                created_at TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0
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
            sequence: 0,
        };
        let result = context.save_event(event).await;
        assert!(result.is_ok());
    }

    // ============================================================
    // Integration tests with real EventStore
    // ============================================================

    #[tokio::test]
    async fn test_persistent_context_creation() {
        let pool = setup_test_db().await;
        let sqlite_store = Arc::new(gasket_storage::SqliteStore::from_pool(pool.clone()));
        let event_store = Arc::new(EventStore::new(pool));

        let context = AgentContext::persistent(event_store, sqlite_store);
        assert!(context.is_persistent());
    }

    #[tokio::test]
    async fn test_persistent_context_save_event() {
        let pool = setup_test_db().await;
        let sqlite_store = Arc::new(gasket_storage::SqliteStore::from_pool(pool.clone()));
        let event_store = Arc::new(EventStore::new(pool));

        let context = AgentContext::persistent(event_store, sqlite_store);

        // Save event
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        let result = context.save_event(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stateless_context_clear_session() {
        let context = AgentContext::Stateless;

        // Clear session should be a no-op for stateless context
        let result = context.clear_session("test:session").await;
        assert!(result.is_ok());
    }
}
