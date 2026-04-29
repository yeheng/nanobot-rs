//! Embedding indexer that builds and maintains the search index.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::index::MemoryIndex;
use crate::provider::EmbeddingProvider;
use crate::vector_store::{VectorRecord, VectorStore};
use gasket_types::{EventType, SessionEvent};

const MIN_CONTENT_LEN: usize = 5;

/// Indexer that computes embeddings and maintains the in-memory search index.
pub struct EmbeddingIndexer {
    cancel: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl EmbeddingIndexer {
    /// Spawn a background task that listens on the broadcast channel for events.
    pub fn start(
        provider: Arc<dyn EmbeddingProvider>,
        store: Arc<dyn VectorStore>,
        index: Arc<MemoryIndex>,
        mut rx: broadcast::Receiver<SessionEvent>,
    ) -> Result<Self> {
        let cancel = CancellationToken::new();
        let cancel_child = cancel.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel_child.cancelled() => break,
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if let Err(e) = Self::process_event(
                                    provider.as_ref(),
                                    store.as_ref(),
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
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });

        Ok(Self {
            cancel,
            handle: Some(handle),
        })
    }

    /// Shut down the background task and wait for it to finish.
    pub async fn shutdown(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
    }

    /// Cold-start: load stored embeddings and insert into the in-memory index.
    ///
    /// When `limit` is `Some(n)`, only the most recent `n` embeddings are loaded
    /// (keeps memory bounded). `None` means load everything.
    pub async fn rebuild_index(
        store: &dyn VectorStore,
        index: &MemoryIndex,
        limit: Option<usize>,
    ) -> Result<usize> {
        let embeddings = match limit {
            Some(n) => store.load_recent(n).await?,
            None => store.load_all().await?,
        };
        let total = embeddings.len();

        for stored in embeddings {
            index.insert(stored.event_id, stored.embedding);
        }

        tracing::info!("rebuild_index: loaded {total} embeddings into index (limit={limit:?})");
        Ok(total)
    }

    /// Process a single event: filter, dedup, embed, persist.
    pub async fn process_event(
        provider: &dyn EmbeddingProvider,
        store: &dyn VectorStore,
        index: &MemoryIndex,
        event: SessionEvent,
    ) -> Result<()> {
        // Only process UserMessage and AssistantMessage.
        let event_type_str = match &event.event_type {
            EventType::UserMessage => "user_message",
            EventType::AssistantMessage => "assistant_message",
            _ => return Ok(()),
        };

        // Skip short content.
        if event.content.len() < MIN_CONTENT_LEN {
            return Ok(());
        }

        // Dedup.
        let event_id = event.id.to_string();
        if store.exists(&event_id).await? {
            return Ok(());
        }

        // Compute content hash + embedding.
        let content_hash = xxhash_rust::xxh3::xxh3_64(event.content.as_bytes()).to_string();
        let embedding = provider.embed(&event.content).await?;

        // Persist to store, then update the hot index.
        store
            .upsert(vec![VectorRecord {
                id: event_id.clone(),
                vector: embedding.clone(),
                session_key: event.session_key.clone(),
                event_type: event_type_str.to_string(),
                content_hash,
            }])
            .await?;

        index.insert(event_id, embedding);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;
    use crate::store::EmbeddingStore;
    use crate::vector_store::VectorStore;
    use chrono::Utc;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    async fn test_store() -> Arc<dyn VectorStore> {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("in-memory pool");
        let store = EmbeddingStore::new(pool, 3);
        store.run_migration().await.expect("migration");
        Arc::new(store)
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
        let index = MemoryIndex::new(3);

        store
            .upsert(vec![
                VectorRecord {
                    id: "evt-1".to_string(),
                    vector: vec![1.0, 0.0, 0.0],
                    session_key: "sess-a".to_string(),
                    event_type: "user_message".to_string(),
                    content_hash: "h1".to_string(),
                },
                VectorRecord {
                    id: "evt-2".to_string(),
                    vector: vec![0.0, 1.0, 0.0],
                    session_key: "sess-a".to_string(),
                    event_type: "assistant_message".to_string(),
                    content_hash: "h2".to_string(),
                },
                VectorRecord {
                    id: "evt-3".to_string(),
                    vector: vec![0.0, 0.0, 1.0],
                    session_key: "sess-a".to_string(),
                    event_type: "user_message".to_string(),
                    content_hash: "h3".to_string(),
                },
            ])
            .await
            .unwrap();

        let count = EmbeddingIndexer::rebuild_index(store.as_ref(), &index, None)
            .await
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(index.len(), 3);

        let results = index.search(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "evt-1");
        assert!(results[0].1 > 0.99);
    }

    #[tokio::test]
    async fn test_process_event_user_message() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = MemoryIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hello, this is a test message");

        EmbeddingIndexer::process_event(&provider, store.as_ref(), &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 1);
    }

    #[tokio::test]
    async fn test_process_event_skips_tool_call() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = MemoryIndex::new(3);

        let event = make_event(
            EventType::ToolCall {
                tool_name: "search".to_string(),
                arguments: serde_json::json!({}),
            },
            "tool call content here",
        );

        EmbeddingIndexer::process_event(&provider, store.as_ref(), &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 0, "tool calls should be skipped");
    }

    #[tokio::test]
    async fn test_process_event_skips_short_content() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = MemoryIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hi");

        EmbeddingIndexer::process_event(&provider, store.as_ref(), &index, event)
            .await
            .unwrap();

        assert_eq!(index.len(), 0, "short content should be skipped");
    }

    #[tokio::test]
    async fn test_process_event_dedup() {
        let provider = MockProvider::new(3);
        let store = test_store().await;
        let index = MemoryIndex::new(3);

        let event = make_event(EventType::UserMessage, "Hello, this is a test message");
        let event_id = event.id.to_string();

        EmbeddingIndexer::process_event(&provider, store.as_ref(), &index, event)
            .await
            .unwrap();
        assert_eq!(index.len(), 1);

        let event2 = SessionEvent {
            id: uuid::Uuid::parse_str(&event_id).unwrap(),
            ..make_event(EventType::UserMessage, "Different content but same ID")
        };

        EmbeddingIndexer::process_event(&provider, store.as_ref(), &index, event2)
            .await
            .unwrap();

        assert_eq!(index.len(), 1);
    }

    #[tokio::test]
    async fn test_start_and_shutdown() {
        let provider = Arc::new(MockProvider::new(3));
        let store = test_store().await;
        let index = Arc::new(MemoryIndex::new(3));
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
        let index = Arc::new(MemoryIndex::new(3));
        let (tx, rx) = broadcast::channel::<SessionEvent>(16);

        let mut indexer =
            EmbeddingIndexer::start(provider, store, index.clone(), rx).expect("start indexer");

        let event = make_event(EventType::UserMessage, "Hello from broadcast channel");
        tx.send(event).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(
            index.len(),
            1,
            "event should be processed by background task"
        );

        indexer.shutdown().await;
    }
}
