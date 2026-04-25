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
use tracing::{debug, info};

use crate::error::AgentError;
use gasket_storage::EventStore;
use gasket_types::SessionKey;
use gasket_types::{Session, SessionEvent};

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
/// Holds references to the event store and sqlite store for session persistence.
/// All fields are populated at construction time — no partial initialization.
#[derive(Clone)]
pub struct PersistentContext {
    /// Event store for persisting events
    pub event_store: Arc<EventStore>,
    /// Session store for summaries and checkpoints
    pub session_store: Arc<gasket_storage::SessionStore>,
}

impl std::fmt::Debug for PersistentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentContext")
            .field("event_store", &"EventStore { .. }")
            .field("session_store", &"SessionStore { .. }")
            .finish()
    }
}

impl AgentContext {
    /// Create a persistent context with event store.
    pub fn persistent(
        event_store: Arc<EventStore>,
        session_store: Arc<gasket_storage::SessionStore>,
    ) -> Self {
        Self::Persistent(PersistentContext {
            event_store,
            session_store,
        })
    }

    /// Check if this context has persistence enabled.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }

    /// Load the summary and its sequence watermark for a session.
    ///
    /// Returns `(summary_text, covered_upto_sequence)`.
    /// For `Stateless` context or if no summary exists, returns `("", 0)`.
    ///
    /// Also loads the latest checkpoint (if any) and merges it into the
    /// summary text so that working-memory snapshots survive restarts.
    /// The watermark is kept at the compaction level because checkpoints
    /// do not delete events.
    pub async fn load_summary_with_watermark(
        &self,
        session_key: &SessionKey,
    ) -> Result<(String, i64), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                let (mut summary, watermark) =
                    match ctx.session_store.load_summary(session_key).await {
                        Ok(Some((content, watermark))) => (content, watermark),
                        Ok(None) => (String::new(), 0),
                        Err(e) => {
                            return Err(AgentError::SessionError(format!(
                                "Failed to load summary for {}: {}",
                                session_key, e
                            )))
                        }
                    };

                // Merge latest checkpoint into summary so working memory
                // survives session restarts.
                let key_str = session_key.to_string();
                if let Ok(Some((ck_summary, _ck_seq))) =
                    ctx.session_store.load_checkpoint(&key_str, i64::MAX).await
                {
                    if !ck_summary.is_empty() {
                        if !summary.is_empty() {
                            summary.push_str("\n\n[Working Memory]\n");
                        }
                        summary.push_str(&ck_summary);
                    }
                }

                Ok((summary, watermark))
            }
            Self::Stateless => Ok((String::new(), 0)),
        }
    }

    /// Load events after a sequence watermark for a session.
    pub async fn get_events_after_watermark(
        &self,
        session_key: &SessionKey,
        watermark: i64,
    ) -> Result<Vec<SessionEvent>, AgentError> {
        match self {
            Self::Persistent(ctx) => {
                let result = if watermark == 0 {
                    ctx.event_store.get_session_history(session_key).await
                } else {
                    ctx.event_store
                        .get_events_after_sequence(session_key, watermark)
                        .await
                };
                result.map_err(|e| {
                    AgentError::SessionError(format!(
                        "Failed to load history for '{}': {}",
                        session_key, e
                    ))
                })
            }
            Self::Stateless => Ok(vec![]),
        }
    }

    /// Load a session for the given key.
    pub async fn load_session(&self, key: &SessionKey) -> Result<Session, AgentError> {
        match self {
            Self::Persistent(ctx) => {
                let events = ctx
                    .event_store
                    .get_session_history(key)
                    .await
                    .map_err(|e| {
                        AgentError::SessionError(format!(
                            "Failed to load session history for '{}': {}",
                            key, e
                        ))
                    })?;

                let mut session = Session::new(key.to_string());
                session.update_from_events(&events);
                debug!("Loaded session {} with {} events", key, events.len());
                Ok(session)
            }
            Self::Stateless => Ok(Session::new(key.to_string())),
        }
    }

    /// Save an event to the session.
    pub async fn save_event(&self, event: SessionEvent) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store
                    .append_event(&event)
                    .await
                    .map_err(|e| AgentError::Other(format!("Failed to persist event: {}", e)))?;

                debug!(
                    "Saved event type={} for session={}",
                    event.event_type.role_str(),
                    event.session_key
                );

                Ok(())
            }
            Self::Stateless => Ok(()),
        }
    }

    /// Clear session data from the event store.
    pub async fn clear_session(&self, session_key: &SessionKey) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store
                    .clear_session(session_key)
                    .await
                    .map_err(|e| AgentError::Other(format!("Failed to clear session: {}", e)))?;

                info!("Cleared session: {}", session_key);
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
        let session = context.load_session(&key).await.expect("load session");
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
            metadata: Default::default(),
            created_at: chrono::Utc::now(),
            sequence: 0,
        };
        let result = context.save_event(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_persistent_context_creation() {
        let pool = setup_test_db().await;
        let session_store = Arc::new(gasket_storage::SessionStore::new(pool.clone()));
        let event_store = Arc::new(EventStore::new(pool));

        let context = AgentContext::persistent(event_store, session_store);
        assert!(context.is_persistent());
    }

    #[tokio::test]
    async fn test_persistent_context_save_event() {
        let pool = setup_test_db().await;
        let session_store = Arc::new(gasket_storage::SessionStore::new(pool.clone()));
        let event_store = Arc::new(EventStore::new(pool));

        let context = AgentContext::persistent(event_store, session_store);

        // Save event
        let event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
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

        let result = context
            .clear_session(&SessionKey::parse("test:session").unwrap())
            .await;
        assert!(result.is_ok());
    }
}
