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

// IndexingQueue, Priority, QueueError are defined below in this file
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

use gasket_storage::SessionStore;
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
    store: Arc<SessionStore>,
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
    pub fn new(store: Arc<SessionStore>) -> Self {
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
    pub fn with_embedder(store: Arc<SessionStore>, embedder: Arc<TextEmbedder>) -> Self {
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
    pub async fn submit(&self, task: AsyncIndexTask, priority: Priority) -> Result<(), QueueError> {
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
        let Some(ref embedder) = self.embedder else {
            return;
        };

        match task {
            AsyncIndexTask::History {
                session_key,
                event_id,
                content,
            } => {
                // Check if already indexed
                if self
                    .store
                    .has_embedding(&event_id.to_string())
                    .await
                    .unwrap_or(false)
                {
                    return;
                }

                match embedder.embed(content) {
                    Ok(embedding) => {
                        let _ = self
                            .store
                            .save_embedding(&event_id.to_string(), session_key, &embedding)
                            .await;
                    }
                    Err(e) => {
                        debug!("Failed to embed history event {}: {}", event_id, e);
                    }
                }
            }
            AsyncIndexTask::Memory { .. } => {
                // memory_embeddings table removed — no-op
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
#[allow(dead_code)]
fn extract_body(content: &str) -> &str {
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            return stripped[end + 3..].trim_start();
        }
    }
    content
}

/// Background worker for processing indexing tasks.
struct IndexingWorker {
    queue: IndexingQueue<AsyncIndexTask>,
    #[allow(dead_code)]
    store: Arc<SessionStore>,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    shutdown: Arc<AtomicBool>,
}

impl IndexingWorker {
    async fn run(&mut self) {
        while !self.shutdown.load(Ordering::Relaxed) {
            match tokio::time::timeout(tokio::time::Duration::from_secs(1), self.queue.pop()).await
            {
                Ok(Some(task)) => self.process_task(&task).await,
                _ => continue, // Timeout or empty queue
            }
        }
    }

    #[cfg(feature = "local-embedding")]
    async fn process_task(&self, task: &AsyncIndexTask) {
        let Some(ref embedder) = self.embedder else {
            return;
        };

        match task {
            AsyncIndexTask::History {
                session_key,
                event_id,
                content,
            } => {
                if self
                    .store
                    .has_embedding(&event_id.to_string())
                    .await
                    .unwrap_or(false)
                {
                    return;
                }

                if let Ok(embedding) = embedder.embed(content) {
                    let _ = self
                        .store
                        .save_embedding(&event_id.to_string(), session_key, &embedding)
                        .await;
                }
            }
            AsyncIndexTask::Memory { .. } => {
                // memory_embeddings table removed — no-op
            }
        }
    }

    #[cfg(not(feature = "local-embedding"))]
    async fn process_task(&self, _task: &AsyncIndexTask) {
        // No-op when local-embedding feature is disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_body_strips_frontmatter() {
        let content = "---\ntitle: Test\n---\n\n# Body content\n\nSome text.";
        assert_eq!("# Body content\n\nSome text.", extract_body(content));
    }

    #[test]
    fn extract_body_no_frontmatter_returns_full() {
        let content = "# Just body\n\nNo frontmatter.";
        assert_eq!("# Just body\n\nNo frontmatter.", extract_body(content));
    }

    #[test]
    fn extract_body_empty_after_frontmatter() {
        let content = "---\ntitle: Test\n---\n";
        assert_eq!("", extract_body(content));
    }

    #[test]
    fn async_index_task_clone() {
        let task = AsyncIndexTask::Memory {
            path: std::path::PathBuf::from("knowledge/test.md"),
            content: "some content".to_string(),
        };
        let cloned = task.clone();

        match cloned {
            AsyncIndexTask::Memory { path, content } => {
                assert_eq!(std::path::PathBuf::from("knowledge/test.md"), path);
                assert_eq!("some content", content);
            }
            _ => panic!("Expected Memory variant"),
        }
    }

    #[test]
    fn async_index_task_history_variant() {
        let id = uuid::Uuid::new_v4();
        let task = AsyncIndexTask::History {
            session_key: "test-session".to_string(),
            event_id: id,
            content: "hello world".to_string(),
        };

        match task {
            AsyncIndexTask::History {
                session_key,
                event_id,
                content,
            } => {
                assert_eq!("test-session", session_key);
                assert_eq!(id, event_id);
                assert_eq!("hello world", content);
            }
            _ => panic!("Expected History variant"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IndexingQueue (from original queue.rs)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::atomic::AtomicUsize;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    P0 = 0, // Real-time (new messages)
    P1 = 1, // Incremental (memory writes)
    P2 = 2, // Batch (backfill, compaction)
}

pub struct IndexingQueue<T> {
    p0_tx: mpsc::Sender<T>,
    p0_rx: mpsc::Receiver<T>,
    p1_tx: mpsc::Sender<T>,
    p1_rx: mpsc::Receiver<T>,
    p2_tx: mpsc::Sender<T>,
    p2_rx: mpsc::Receiver<T>,
    depth: Arc<AtomicUsize>,
    max_depth: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Queue is full")]
    Full,
}

impl<T> IndexingQueue<T> {
    pub fn new(max_depth: usize) -> Self {
        let per_queue = max_depth / 3;
        let (p0_tx, p0_rx) = mpsc::channel(per_queue);
        let (p1_tx, p1_rx) = mpsc::channel(per_queue);
        let (p2_tx, p2_rx) = mpsc::channel(per_queue);

        Self {
            p0_tx,
            p0_rx,
            p1_tx,
            p1_rx,
            p2_tx,
            p2_rx,
            depth: Arc::new(AtomicUsize::new(0)),
            max_depth,
        }
    }

    pub async fn push(&self, item: T, priority: Priority) -> Result<(), QueueError> {
        let tx = match priority {
            Priority::P0 => &self.p0_tx,
            Priority::P1 => &self.p1_tx,
            Priority::P2 => &self.p2_tx,
        };

        match tx.try_send(item) {
            Ok(_) => {
                self.depth.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => Err(QueueError::Full),
        }
    }

    pub async fn pop(&mut self) -> Option<T> {
        let result = tokio::select! {
            biased;
            item = self.p0_rx.recv() => item,
            item = self.p1_rx.recv() => item,
            item = self.p2_rx.recv() => item,
        };

        if result.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }

        result
    }

    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    #[tokio::test]
    async fn priority_ordering_p0_before_p1_and_p2() {
        let mut queue = IndexingQueue::<&str>::new(300);

        // Push in reverse priority order
        queue.push("low", Priority::P2).await.unwrap();
        queue.push("mid", Priority::P1).await.unwrap();
        queue.push("high", Priority::P0).await.unwrap();

        // P0 should come out first
        assert_eq!(Some("high"), queue.pop().await);
        assert_eq!(Some("mid"), queue.pop().await);
        assert_eq!(Some("low"), queue.pop().await);
    }

    #[tokio::test]
    async fn depth_tracking() {
        let mut queue = IndexingQueue::<i32>::new(300);

        assert_eq!(0, queue.depth());

        queue.push(1, Priority::P0).await.unwrap();
        queue.push(2, Priority::P1).await.unwrap();
        queue.push(3, Priority::P2).await.unwrap();

        assert_eq!(3, queue.depth());

        queue.pop().await;
        assert_eq!(2, queue.depth());

        queue.pop().await;
        queue.pop().await;
        assert_eq!(0, queue.depth());
    }

    #[tokio::test]
    async fn queue_full_returns_error() {
        // max_depth=3 → per_queue=1 (each priority channel holds 1 item)
        let queue = IndexingQueue::<i32>::new(3);

        assert!(queue.push(1, Priority::P0).await.is_ok());
        // Second push to P0 should fail (channel capacity = 1)
        assert!(matches!(
            queue.push(2, Priority::P0).await,
            Err(QueueError::Full)
        ));
    }

    #[tokio::test]
    async fn fifo_ordering_within_same_priority() {
        let mut queue = IndexingQueue::<&str>::new(300);

        queue.push("first", Priority::P1).await.unwrap();
        queue.push("second", Priority::P1).await.unwrap();

        assert_eq!(Some("first"), queue.pop().await);
        assert_eq!(Some("second"), queue.pop().await);
    }
}
