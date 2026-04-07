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
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::task::JoinHandle;
use crate::agent::indexing_queue::{IndexingQueue, Priority, QueueError};

use gasket_storage::SqliteStore;
use gasket_types::SessionEvent;

#[cfg(feature = "local-embedding")]
use {gasket_storage::TextEmbedder, tracing::debug};

/// Async indexing task for queue-based processing
#[derive(Debug, Clone)]
pub enum AsyncIndexTask {
    /// Index a single history event
    History {
        session_key: String,
        event_id: uuid::Uuid,
        content: String,
    },
    /// Index a memory file
    Memory {
        path: std::path::PathBuf,
        content: String,
    },
}

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
    /// Async task queue (optional, set via enable_queue)
    queue: Option<Arc<IndexingQueue<AsyncIndexTask>>>,
    /// Shutdown signal for background worker
    shutdown: Arc<AtomicBool>,
    /// Background worker handle
    worker_handle: Option<JoinHandle<()>>,
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
            queue: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            worker_handle: None,
        }
    }

    /// Create with an embedder for semantic indexing.
    #[cfg(feature = "local-embedding")]
    pub fn with_embedder(store: Arc<SqliteStore>, embedder: Arc<TextEmbedder>) -> Self {
        Self {
            store,
            embedder: Some(embedder),
            queue: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            worker_handle: None,
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

    /// Enable async queue processing with the specified max depth.
    pub fn enable_queue(&mut self, max_depth: usize) {
        let queue = Arc::new(IndexingQueue::new(max_depth));
        self.queue = Some(queue);
    }

    /// Start the background worker.
    ///
    /// # Panics
    /// Panics if called without first calling `enable_queue`.
    pub fn start_worker(&mut self) {
        let Some(queue) = self.queue.take() else {
            panic!("enable_queue must be called before start_worker");
        };

        let store = self.store.clone();
        #[cfg(feature = "local-embedding")]
        let embedder = self.embedder.clone();
        let shutdown = self.shutdown.clone();

        let handle = tokio::spawn(async move {
            // Unwrap the Arc to take ownership of the queue
            let queue = Arc::try_unwrap(queue).unwrap_or_else(|q| {
                // If we can't unwrap (shouldn't happen), create a new empty queue
                IndexingQueue::new(q.max_depth())
            });
            let mut worker = IndexingWorker {
                queue,
                store,
                #[cfg(feature = "local-embedding")]
                embedder,
                shutdown,
            };
            worker.run().await;
        });

        self.worker_handle = Some(handle);
    }

    /// Submit an async indexing task.
    ///
    /// If the queue is not enabled, processes synchronously.
    /// If shutdown is in progress, silently drops the task.
    pub async fn submit(
        &self,
        task: AsyncIndexTask,
        priority: Priority,
    ) -> Result<(), QueueError> {
        let Some(ref queue) = self.queue else {
            // Queue not enabled, process synchronously
            self.process_task(&task).await;
            return Ok(());
        };

        if self.shutdown.load(Ordering::Relaxed) {
            return Ok(()); // Graceful degradation during shutdown
        }

        queue.push(task, priority).await
    }

    /// Process a single task synchronously.
    #[cfg(feature = "local-embedding")]
    async fn process_task(&self, task: &AsyncIndexTask) {
        let Some(ref embedder) = self.embedder else { return };

        match task {
            AsyncIndexTask::History { session_key, event_id, content } => {
                // Check if already indexed
                if self.store.has_embedding(&event_id.to_string()).await.unwrap_or(false) {
                    return;
                }

                match embedder.embed(content) {
                    Ok(embedding) => {
                        let _ = self.store.save_embedding(
                            &event_id.to_string(),
                            session_key,
                            &embedding
                        ).await;
                    }
                    Err(e) => {
                        debug!("Failed to embed history event {}: {}", event_id, e);
                    }
                }
            }
            AsyncIndexTask::Memory { path, content } => {
                // Extract body without frontmatter
                let body = extract_body(content);
                if body.len() < 10 { return; }

                match embedder.embed(&body) {
                    Ok(embedding) => {
                        let path_str = path.to_string_lossy();
                        // Save to memory_embeddings table
                        let _ = sqlx::query(
                            "INSERT OR REPLACE INTO memory_embeddings
                             (memory_path, scenario, embedding, token_count, updated_at)
                             VALUES (?1, 'general', ?2, ?3, datetime('now'))"
                        )
                        .bind(path_str.as_ref())
                        .bind(bytemuck::cast_slice(&embedding))
                        .bind(body.len() as i64)
                        .execute(&self.store.pool())
                        .await;
                    }
                    Err(e) => {
                        debug!("Failed to embed memory {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "local-embedding"))]
    async fn process_task(&self, _task: &AsyncIndexTask) {
        // No-op when local-embedding feature is disabled
    }

    /// Signal shutdown and wait for worker to complete.
    pub async fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker_handle.take() {
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await;
        }
    }
}

/// Extract body content from a memory file, stripping frontmatter.
fn extract_body(content: &str) -> &str {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return &content[end + 6..].trim_start();
        }
    }
    content
}

/// Background worker for processing indexing tasks.
struct IndexingWorker {
    queue: IndexingQueue<AsyncIndexTask>,
    store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    shutdown: Arc<AtomicBool>,
}

impl IndexingWorker {
    async fn run(&mut self) {
        while !self.shutdown.load(Ordering::Relaxed) {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(1),
                self.queue.pop()
            ).await {
                Ok(Some(task)) => self.process_task(&task).await,
                _ => continue, // Timeout or empty queue
            }
        }
    }

    #[cfg(feature = "local-embedding")]
    async fn process_task(&self, task: &AsyncIndexTask) {
        let Some(ref embedder) = self.embedder else { return };

        match task {
            AsyncIndexTask::History { session_key, event_id, content } => {
                if self.store.has_embedding(&event_id.to_string()).await.unwrap_or(false) {
                    return;
                }

                if let Ok(embedding) = embedder.embed(content) {
                    let _ = self.store.save_embedding(
                        &event_id.to_string(),
                        session_key,
                        &embedding
                    ).await;
                }
            }
            AsyncIndexTask::Memory { path, content } => {
                let body = extract_body(content);
                if body.len() < 10 { return; }

                if let Ok(embedding) = embedder.embed(&body) {
                    let path_str = path.to_string_lossy();
                    let _ = sqlx::query(
                        "INSERT OR REPLACE INTO memory_embeddings
                         (memory_path, scenario, embedding, token_count, updated_at)
                         VALUES (?1, 'general', ?2, ?3, datetime('now'))"
                    )
                    .bind(path_str.as_ref())
                    .bind(bytemuck::cast_slice(&embedding))
                    .bind(body.len() as i64)
                    .execute(&self.store.pool())
                    .await;
                }
            }
        }
    }

    #[cfg(not(feature = "local-embedding"))]
    async fn process_task(&self, _task: &AsyncIndexTask) {
        // No-op when local-embedding feature is disabled
    }
}
