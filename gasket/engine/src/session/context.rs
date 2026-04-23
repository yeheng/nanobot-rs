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
use tracing::{debug, info, warn};

use crate::error::AgentError;
use gasket_storage::EventStore;
use gasket_types::SessionKey;
use gasket_types::{Session, SessionEvent};

#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;

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
    /// Session store for summaries, checkpoints, and embeddings
    pub session_store: Arc<gasket_storage::SessionStore>,
    /// Optional text embedder for synchronous embedding on save.
    /// When present, every saved event gets an embedding for semantic recall.
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}

impl std::fmt::Debug for PersistentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentContext")
            .field("event_store", &"EventStore { .. }")
            .field("session_store", &"SessionStore { .. }")
            .field("embedder", &"Option<TextEmbedder> { .. }")
            .finish()
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
        session_store: Arc<gasket_storage::SessionStore>,
    ) -> Self {
        Self::Persistent(PersistentContext {
            event_store,
            session_store,
            #[cfg(feature = "local-embedding")]
            embedder: None,
        })
    }

    /// Create a persistent context with embedder for semantic indexing.
    #[cfg(feature = "local-embedding")]
    pub fn persistent_with_embedder(
        event_store: Arc<EventStore>,
        session_store: Arc<gasket_storage::SessionStore>,
        embedder: Arc<gasket_storage::TextEmbedder>,
    ) -> Self {
        Self::Persistent(PersistentContext {
            event_store,
            session_store,
            embedder: Some(embedder),
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
    ///
    /// Returns only events with `sequence > watermark`, i.e., events not yet
    /// covered by the summary. For `Stateless` context, returns empty vector.
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
    ///
    /// For `Persistent` context, loads events from EventStore and reconstructs session state.
    /// For `Stateless` context, creates a new in-memory session.
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
    ///
    /// For `Persistent` context, persists the event to the EventStore and
    ///同步生成 embedding for semantic recall (if embedder is configured).
    /// For `Stateless` context, this is a no-op.
    pub async fn save_event(&self, event: SessionEvent) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                // Save event to EventStore
                ctx.event_store
                    .append_event(&event)
                    .await
                    .map_err(|e| AgentError::Other(format!("Failed to persist event: {}", e)))?;

                debug!(
                    "Saved event type={} for session={}",
                    event.event_type.role_str(),
                    event.session_key
                );

                // Synchronously generate and save embedding (if embedder is configured)
                #[cfg(feature = "local-embedding")]
                if let Some(ref embedder) = ctx.embedder {
                    let event_id = event.id.to_string();
                    let session_key = event.session_key.clone();
                    match embedder.embed(&event.content) {
                        Ok(embedding) => {
                            if let Err(e) = ctx
                                .session_store
                                .save_embedding(&event_id, &session_key, &embedding)
                                .await
                            {
                                warn!("Failed to save embedding for event {}: {}", event_id, e);
                            } else {
                                debug!(
                                    "Saved embedding for event {} in session {}",
                                    event_id, session_key
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to embed event {}: {}", event_id, e);
                        }
                    }
                }

                Ok(())
            }
            Self::Stateless => Ok(()),
        }
    }

    /// Recall relevant historical messages using semantic embedding similarity.
    ///
    /// Returns the top-K most relevant messages based on cosine similarity
    /// between the query embedding and stored session embeddings.
    /// Falls back to recency scoring if no embeddings are available.
    /// For `Stateless` context, returns an empty vector.
    pub async fn recall_history(
        &self,
        key: &SessionKey,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<String>, AgentError> {
        match self {
            Self::Persistent(ctx) => {
                // Fallback: if no embedder or empty query, return empty
                if query_embedding.is_empty() {
                    return Ok(Vec::new());
                }

                let key_str = key.to_string();

                // Load pre-computed embeddings for this session
                let embeddings = match ctx.session_store.load_embeddings(&key_str).await {
                    Ok(embs) => embs,
                    Err(e) => {
                        warn!("Failed to load session embeddings for recall: {}", e);
                        return Ok(Vec::new());
                    }
                };

                if embeddings.is_empty() {
                    debug!(
                        "No embeddings found for session {}, returning empty recall",
                        key
                    );
                    return Ok(Vec::new());
                }

                // Prepare candidates for top-k search
                let candidates: Vec<(String, Vec<f32>)> = embeddings
                    .iter()
                    .map(|(_, content, emb)| (content.clone(), emb.clone()))
                    .collect();

                // Find top-K similar messages using cosine similarity
                let top_results =
                    gasket_storage::top_k_similar(query_embedding, &candidates, top_k);

                if top_results.is_empty() {
                    debug!("No similar messages found for session {}", key);
                    return Ok(Vec::new());
                }

                let results: Vec<String> = top_results
                    .into_iter()
                    .map(|(content, _score)| content.to_string())
                    .collect();
                debug!(
                    "Recalled {} history items for {} (top_k={}, candidates={})",
                    results.len(),
                    key,
                    top_k,
                    candidates.len()
                );
                Ok(results)
            }
            Self::Stateless => Ok(Vec::new()),
        }
    }

    /// Clear session data from the event store.
    ///
    /// For `Persistent` context, clears all events and session data from the EventStore,
    /// and also removes the evolution watermark from SqliteStore to keep machine state consistent.
    /// For `Stateless` context, this is a no-op.
    ///
    /// # Arguments
    ///
    /// * `session_key` - The session key to clear
    ///
    /// # Errors
    ///
    /// Returns an error if the session cannot be cleared from the database.
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

    // ============================================================
    // Integration tests with real EventStore
    // ============================================================

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

        // Clear session should be a no-op for stateless context
        let result = context
            .clear_session(&SessionKey::parse("test:session").unwrap())
            .await;
        assert!(result.is_ok());
    }
}
