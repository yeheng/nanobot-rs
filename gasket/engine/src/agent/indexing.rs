//! Semantic indexing service — decoupled from summarization.
//!
//! Handles embedding generation for evicted (and any other) events.
//! Runs independently from compaction/summarization so that semantic
//! indexing succeeds even if the LLM summarization call fails.
//!
//! # Design
//!
//! Embedding is a **write-path concern**: every event that passes through
//! `PersistentContext::save_event()` already gets auto-embedded. This service
//! acts as a safety net for evicted events that may not have embeddings
//! (e.g. events created before the embedder was configured).

use std::sync::Arc;

use gasket_storage::SqliteStore;
use gasket_types::SessionEvent;

#[cfg(feature = "local-embedding")]
use {gasket_storage::TextEmbedder, tracing::debug};

/// Semantic indexing service for conversation events.
///
/// Generates and persists vector embeddings for events, enabling
/// semantic history recall. Decoupled from `ContextCompactor`
/// so that indexing and summarization can fail independently.
#[allow(dead_code)]
pub struct IndexingService {
    /// SQLite store for persisting embeddings.
    store: Arc<SqliteStore>,
    /// Optional text embedder (gated by `local-embedding` feature).
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
}

impl IndexingService {
    /// Create a new indexing service without an embedder.
    ///
    /// Calls to `index_events` will be no-ops until an embedder is set.
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self {
            store,
            #[cfg(feature = "local-embedding")]
            embedder: None,
        }
    }

    /// Create with an embedder for semantic indexing.
    #[cfg(feature = "local-embedding")]
    pub fn with_embedder(store: Arc<SqliteStore>, embedder: Arc<TextEmbedder>) -> Self {
        Self {
            store,
            embedder: Some(embedder),
        }
    }

    /// Set or replace the embedder at runtime.
    #[cfg(feature = "local-embedding")]
    pub fn set_embedder(&mut self, embedder: Arc<TextEmbedder>) {
        self.embedder = Some(embedder);
    }

    /// Generate and store embeddings for the given events.
    ///
    /// Events that already have embeddings in the store are skipped.
    /// This is a safety net — most events are already embedded at save time
    /// via `PersistentContext::save_event()`.
    ///
    /// # Errors
    ///
    /// Errors are logged but not propagated. A failed embedding must not
    /// block the response pipeline.
    pub async fn index_events(&self, session_key: &str, events: &[SessionEvent]) {
        #[cfg(not(feature = "local-embedding"))]
        {
            let _ = (session_key, events);
        }

        #[cfg(feature = "local-embedding")]
        {
            let Some(ref embedder) = self.embedder else {
                debug!("No embedder configured, skipping evicted event indexing");
                return;
            };

            if events.is_empty() {
                return;
            }

            // Phase 1: filter out events that already have embeddings
            let mut to_embed: Vec<&SessionEvent> = Vec::new();
            for event in events {
                let event_id = event.id.to_string();
                match self.store.has_embedding(&event_id).await {
                    Ok(true) => {
                        debug!("Embedding already exists for event {}, skipping", event_id);
                    }
                    Ok(false) => to_embed.push(event),
                    Err(e) => {
                        debug!("Failed to check existing embedding for {}: {}", event_id, e);
                        to_embed.push(event); // try anyway
                    }
                }
            }

            if to_embed.is_empty() {
                return;
            }

            // Phase 2: batch embed all new events
            let texts: Vec<String> = to_embed.iter().map(|e| e.content.clone()).collect();
            match embedder.embed_batch(&texts) {
                Ok(embeddings) => {
                    for (event, embedding) in to_embed.into_iter().zip(embeddings) {
                        let event_id = event.id.to_string();
                        if let Err(e) = self
                            .store
                            .save_embedding(&event_id, session_key, &embedding)
                            .await
                        {
                            debug!(
                                "Failed to save embedding for evicted event {}: {}",
                                event_id, e
                            );
                        } else {
                            debug!(
                                "Saved embedding for evicted event {} in session {}",
                                event_id, session_key
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!("Batch embedding failed for {} events: {}", texts.len(), e);
                }
            }
        }
    }
}
