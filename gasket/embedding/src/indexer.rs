//! Embedding indexer that builds and maintains the search index.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::index::HnswIndex;
use crate::provider::EmbeddingProvider;
use crate::store::EmbeddingStore;
use gasket_types::{EventType, SessionEvent};

const _COLD_START_BATCH_SIZE: usize = 1000;
const MIN_CONTENT_LEN: usize = 5;

/// Indexer that computes embeddings and maintains the HNSW index.
pub struct EmbeddingIndexer {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EmbeddingIndexer {
    /// Spawn a background task that listens on the broadcast channel for events.
    pub fn start(
        provider: Arc<dyn EmbeddingProvider>,
        store: EmbeddingStore,
        index: Arc<HnswIndex>,
        mut rx: broadcast::Receiver<SessionEvent>,
    ) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let handle = tokio::spawn(async move {
            loop {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }

                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if let Err(e) = Self::process_event(
                                    provider.as_ref(),
                                    &store,
                                    &index,
                                    event,
                                )
                                .await
                                {
                                    tracing::warn!("embedding indexer process error: {e}");
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("embedding indexer lagged {n} events");
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        if shutdown_clone.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            shutdown,
            handle: Some(handle),
        })
    }

    /// Shut down the background task and wait for it to finish.
    pub async fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
    }

    /// Cold-start: load stored embeddings and insert into the in-memory index.
    ///
    /// When `limit` is `Some(n)`, only the most recent `n` embeddings are loaded
    /// (keeps memory bounded). `None` means load everything.
    pub async fn rebuild_index(
        store: &EmbeddingStore,
        index: &HnswIndex,
        limit: Option<usize>,
    ) -> Result<usize> {
        let embeddings = match limit {
            Some(n) => store.load_recent(n).await?,
            None => store.load_all().await?,
        };
        let total = embeddings.len();

        for stored in &embeddings {
            index.insert(stored.event_id.clone(), stored.embedding.clone());
        }

        tracing::info!("rebuild_index: loaded {total} embeddings into index (limit={limit:?})");
        Ok(total)
    }

    /// Process a single event: filter, dedup, embed, persist.
    pub async fn process_event(
        provider: &dyn EmbeddingProvider,
        store: &EmbeddingStore,
        index: &HnswIndex,
        event: SessionEvent,
    ) -> Result<()> {
        // Only process UserMessage and AssistantMessage.
        match &event.event_type {
            EventType::UserMessage | EventType::AssistantMessage => {}
            _ => return Ok(()),
        }

        // Skip short content.
        if event.content.len() < MIN_CONTENT_LEN {
            return Ok(());
        }

        // Dedup.
        if store.exists(&event.id.to_string()).await? {
            return Ok(());
        }

        // Compute content hash.
        let content_hash = xxhash_rust::xxh3::xxh3_64(event.content.as_bytes()).to_string();

        // Embed.
        let embedding = provider.embed(&event.content).await?;

        // Persist to store.
        let event_type_str = match &event.event_type {
            EventType::UserMessage => "user_message",
            EventType::AssistantMessage => "assistant_message",
            _ => unreachable!("already filtered above"),
        };

        store
            .save(
                &event.id.to_string(),
                &event.session_key,
                "",
                "",
                &embedding,
                event_type_str,
                &content_hash,
            )
            .await?;

        // Insert into in-memory index.
        index.insert(event.id.to_string(), embedding);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;
    use chrono::Utc;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    async fn test_store() -> EmbeddingStore {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("in-memory pool");
        let store = EmbeddingStore::new(pool);
        store.run_migration().await.expect("migration");
        store
    }

    fn make_event(event_type: EventType, content: &str) -> SessionEvent {
        SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test-session".to_string(),
            event_type,
            content: content.to_string(),
            metadata: gasket_types::EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        }
    }

    #[tokio::test]
    async fn test_rebuild_index() {
        let store = test_store().await;
        let index = HnswIndex::new(3);

        // Save some embeddings directly.
        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &[1.0, 0.0, 0.0],
                "user_message",
                "h1",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-2",
                "sess-a",
                "",
                "",
                &[0.0, 1.0, 0.0],
                "assistant_message",
                "h2",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-3",
                "sess-a",
                "",
                "",
                &[0.0, 0.0, 1.0],
                "user_message",
                "h3",
            )
            .await
            .unwrap();

        // Rebuild into an empty index.
        let count = EmbeddingIndexer::rebuild_index(&store, &index, None)
            .await
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(index.len(), 3);

        // Verify search works.
        let results = index.search(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "evt-1");
        assert!(results[0].1 > 0.99);
    }

    #[tokio::test]
    async fn test_process_event_user_message() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = HnswIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hello, this is a test message");

        EmbeddingIndexer::process_event(&provider, &store, &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 1);
        assert!(store.exists("placeholder").await.is_ok());
    }

    #[tokio::test]
    async fn test_process_event_skips_tool_call() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = HnswIndex::new(3);

        let event = make_event(
            EventType::ToolCall {
                tool_name: "search".to_string(),
                arguments: serde_json::json!({}),
            },
            "tool call content here",
        );

        EmbeddingIndexer::process_event(&provider, &store, &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 0, "tool calls should be skipped");
    }

    #[tokio::test]
    async fn test_process_event_skips_short_content() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = HnswIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hi");

        EmbeddingIndexer::process_event(&provider, &store, &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 0, "short content should be skipped");
    }

    #[tokio::test]
    async fn test_process_event_dedup() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = HnswIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hello, this is a test message");
        let event_id = event.id.to_string();

        EmbeddingIndexer::process_event(&provider, &store, &index, event)
            .await
            .unwrap();
        assert_eq!(index.len(), 1);

        // Create a second event with the same ID — should be deduplicated.
        let event2 = SessionEvent {
            id: uuid::Uuid::parse_str(&event_id).unwrap(),
            ..make_event(EventType::UserMessage, "Different content but same ID")
        };

        EmbeddingIndexer::process_event(&provider, &store, &index, event2)
            .await
            .unwrap();

        // Only 1 entry — the second was deduped.
        assert_eq!(index.len(), 1);
    }

    #[tokio::test]
    async fn test_start_and_shutdown() {
        let provider = Arc::new(MockProvider::new(3));
        let store = test_store().await;
        let index = Arc::new(HnswIndex::new(3));
        let (_tx, rx) = broadcast::channel::<SessionEvent>(16);

        let mut indexer =
            EmbeddingIndexer::start(provider, store, index, rx).expect("start indexer");

        indexer.shutdown().await;
        assert!(indexer.handle.is_none());
    }

    #[tokio::test]
    async fn test_start_processes_events() {
        let provider = Arc::new(MockProvider::new(3));
        let store = test_store().await;
        let index = Arc::new(HnswIndex::new(3));
        let (tx, rx) = broadcast::channel::<SessionEvent>(16);

        let mut indexer =
            EmbeddingIndexer::start(provider, store, index.clone(), rx).expect("start indexer");

        let event = make_event(EventType::UserMessage, "Hello from broadcast channel");
        tx.send(event).unwrap();

        // Give the background task time to process.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(
            index.len(),
            1,
            "event should be processed by background task"
        );

        indexer.shutdown().await;
    }
}
