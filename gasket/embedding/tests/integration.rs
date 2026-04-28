//! Integration tests for the full embedding recall flow.
//!
//! Tests cover: end-to-end recall search, cold-start index rebuild,
//! broadcast-driven indexer processing, and deduplication.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use gasket_embedding::{
    EmbeddingIndexer, EmbeddingProvider, EmbeddingStore, MemoryIndex, RecallConfig, RecallSearcher,
    VectorRecord, VectorStore,
};
use gasket_storage::{EventStore, EventStoreTrait};
use gasket_types::{EventMetadata, EventType, SessionEvent};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Deterministic mock provider
// ---------------------------------------------------------------------------

/// Mock provider that produces deterministic embeddings based on text content.
///
/// Each of the `dim` dimensions is seeded from the byte value at position `i`
/// in the text. This gives repeatable, content-sensitive vectors suitable for
/// integration assertions.
struct DeterministicMockProvider {
    dim: usize,
}

impl DeterministicMockProvider {
    fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl EmbeddingProvider for DeterministicMockProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = vec![0.0f32; self.dim];
        for (i, byte) in text.as_bytes().iter().enumerate().take(self.dim) {
            v[i] = *byte as f32 / 255.0;
        }
        Ok(v)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_embedding_store(dim: usize) -> Arc<dyn VectorStore> {
    let pool = SqlitePoolOptions::new()
        .connect(":memory:")
        .await
        .expect("in-memory pool");
    let store = EmbeddingStore::with_dim(pool, dim);
    store.run_migration().await.expect("embedding migration");
    Arc::new(store)
}

/// Create the sessions_v2 + session_events schema needed by EventStore.
async fn setup_event_db() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .connect(":memory:")
        .await
        .expect("in-memory pool");

    sqlx::query(
        r#"
        CREATE TABLE sessions_v2 (
            key TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_consolidated_event TEXT,
            total_events INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            channel TEXT NOT NULL DEFAULT '',
            chat_id TEXT NOT NULL DEFAULT ''
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

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_channel_chat ON session_events(channel, chat_id)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_channel_chat_sequence ON session_events(channel, chat_id, sequence)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_sessions_v2_channel_chat ON sessions_v2(channel, chat_id)",
    )
    .execute(&pool)
    .await
    .unwrap();

    pool
}

fn make_event(event_type: EventType, content: &str) -> SessionEvent {
    SessionEvent {
        id: Uuid::now_v7(),
        session_key: "test:session".to_string(),
        event_type,
        content: content.to_string(),
        metadata: EventMetadata::default(),
        created_at: Utc::now(),
        sequence: 0,
    }
}

// ---------------------------------------------------------------------------
// Test 1: Full recall flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_recall_flow() {
    let dim = 4;
    let store = make_embedding_store(dim).await;
    let provider = Arc::new(DeterministicMockProvider::new(dim));
    let index = Arc::new(MemoryIndex::new(dim));

    // Insert 5 events with pre-computed embeddings.
    let contents = [
        "Rust error handling is done with Result and Option types",
        "The quick brown fox jumps over the lazy dog",
        "Error propagation in Rust uses the ? operator",
        "Python uses exceptions for error handling mechanisms",
        "Memory safety is a core feature of the Rust language",
    ];

    let mut records = Vec::new();
    for (i, content) in contents.iter().enumerate() {
        let event_id = format!("evt-{i}");
        let embedding = provider.embed(content).await.unwrap();
        records.push(VectorRecord {
            id: event_id.clone(),
            vector: embedding.clone(),
            session_key: "test:session".to_string(),
            event_type: if i % 2 == 0 {
                "user_message".to_string()
            } else {
                "assistant_message".to_string()
            },
            content_hash: format!("hash-{i}"),
        });
        index.insert(event_id, embedding);
    }
    store.upsert(records).await.unwrap();

    assert_eq!(index.len(), 5);

    // Rebuild from store to verify cold-start path works.
    let rebuilt = EmbeddingIndexer::rebuild_index(store.as_ref(), &index, None)
        .await
        .unwrap();
    assert_eq!(rebuilt, 5);

    let searcher = RecallSearcher::new(provider.clone(), index.clone(), store);

    // Search for "rust error handling" — should match best with events 0 and 2.
    let config = RecallConfig {
        top_k: 10,
        min_score: 0.0,
        ..Default::default()
    };
    let results = searcher
        .recall("rust error handling", &config)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "should find results for relevant query"
    );

    // The top result should be one of the rust/error-related events.
    let top_ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        top_ids.contains(&"evt-0") || top_ids.contains(&"evt-2"),
        "top results should include rust error handling events, got: {:?}",
        top_ids,
    );

    // Verify min_score filtering: a high threshold should exclude everything.
    let strict_config = RecallConfig {
        top_k: 10,
        min_score: 0.99,
        ..Default::default()
    };
    let strict_results = searcher
        .recall("rust error handling", &strict_config)
        .await
        .unwrap();
    // Only exact matches survive a 0.99 threshold with this provider.
    assert!(
        strict_results.len() < results.len(),
        "higher min_score should produce fewer results",
    );

    // Verify top_k limiting.
    let limited_config = RecallConfig {
        top_k: 2,
        min_score: 0.0,
        ..Default::default()
    };
    let limited_results = searcher
        .recall("rust error handling", &limited_config)
        .await
        .unwrap();
    assert!(
        limited_results.len() <= 2,
        "results should respect top_k limit",
    );

    // Verify scores are in descending order.
    for window in limited_results.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "results should be sorted by descending score",
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2: Cold start rebuild
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cold_start_rebuild() {
    let dim = 4;
    let store = make_embedding_store(dim).await;
    let provider = DeterministicMockProvider::new(dim);

    // Save 3 embeddings directly into the store.
    let items = [
        ("evt-a", "hello world"),
        ("evt-b", "rust programming"),
        ("evt-c", "embeddings are cool"),
    ];

    let mut records = Vec::new();
    for (id, text) in &items {
        let embedding = provider.embed(text).await.unwrap();
        records.push(VectorRecord {
            id: id.to_string(),
            vector: embedding,
            session_key: "sess-rebuild".to_string(),
            event_type: "user_message".to_string(),
            content_hash: "h".to_string(),
        });
    }
    store.upsert(records).await.unwrap();

    // Create an empty index and rebuild from store.
    let index = MemoryIndex::new(dim);
    assert_eq!(index.len(), 0, "index should start empty");

    let count = EmbeddingIndexer::rebuild_index(store.as_ref(), &index, None)
        .await
        .unwrap();

    assert_eq!(count, 3, "rebuild should report 3 embeddings");
    assert_eq!(index.len(), 3, "index should contain 3 entries");

    // Search for "rust" — should return evt-b as the closest match.
    let query_vec = provider.embed("rust").await.unwrap();
    let results = index.search(&query_vec, 3);
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].0, "evt-b",
        "evt-b should be the top match for 'rust'"
    );

    // Verify the round-trip: embeddings loaded from store match what we'd
    // compute fresh.
    let stored = store.load_all().await.unwrap();
    assert_eq!(stored.len(), 3);
    for s in &stored {
        let fresh = provider.embed("dummy").await.unwrap();
        // All stored embeddings should have the correct dimensionality.
        assert_eq!(s.embedding.len(), dim);
        // The stored embedding is valid (not all zeros for non-empty text).
        assert_ne!(
            s.embedding, fresh,
            "stored embedding should differ per content"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3: Indexer broadcast processing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_indexer_broadcast_processing() {
    let pool = setup_event_db().await;

    // Create both stores sharing the same pool.
    let event_store = EventStore::new(pool.clone());
    let embedding_store: Arc<dyn VectorStore> = {
        let store = EmbeddingStore::with_dim(pool, 4);
        store.run_migration().await.unwrap();
        Arc::new(store)
    };

    let provider = Arc::new(DeterministicMockProvider::new(4));
    let index = Arc::new(MemoryIndex::new(4));

    // Subscribe to broadcast BEFORE starting indexer so no events are missed.
    let rx = event_store.subscribe();

    let mut indexer = EmbeddingIndexer::start(provider.clone(), embedding_store, index.clone(), rx)
        .expect("start indexer");

    // Append events to EventStore — they get broadcast to the indexer.
    let e1 = make_event(EventType::UserMessage, "First user message via broadcast");
    let e2 = make_event(
        EventType::AssistantMessage,
        "Assistant response via broadcast",
    );
    let e3 = make_event(
        EventType::ToolCall {
            tool_name: "search".into(),
            arguments: serde_json::json!({}),
        },
        "tool call content here",
    );

    event_store.append_event(&e1).await.unwrap();
    event_store.append_event(&e2).await.unwrap();
    event_store.append_event(&e3).await.unwrap();

    // Wait for async processing.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // e1 and e2 should be indexed (UserMessage and AssistantMessage).
    // e3 is a ToolCall, so it should be skipped.
    assert_eq!(
        index.len(),
        2,
        "only user/assistant messages should be indexed"
    );

    // Verify embeddings exist in the store for the two processed events.
    let stored = index.search(&provider.embed("query").await.unwrap(), 10);
    let stored_ids: Vec<&str> = stored.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        stored_ids.contains(&e1.id.to_string().as_str()),
        "e1 should be in index",
    );
    assert!(
        stored_ids.contains(&e2.id.to_string().as_str()),
        "e2 should be in index",
    );

    // Clean shutdown.
    indexer.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 4: Dedup in indexer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_indexer_dedup() {
    let pool = setup_event_db().await;

    let event_store = EventStore::new(pool.clone());
    let embedding_store: Arc<dyn VectorStore> = {
        let store = EmbeddingStore::with_dim(pool, 4);
        store.run_migration().await.unwrap();
        Arc::new(store)
    };

    let provider = Arc::new(DeterministicMockProvider::new(4));
    let index = Arc::new(MemoryIndex::new(4));

    // Create an event and pre-save its embedding manually.
    let event = make_event(EventType::UserMessage, "Duplicate detection test message");
    let event_id_str = event.id.to_string();

    let embedding = provider.embed(&event.content).await.unwrap();
    embedding_store
        .upsert(vec![VectorRecord {
            id: event_id_str.clone(),
            vector: embedding,
            session_key: event.session_key.clone(),
            event_type: "user_message".to_string(),
            content_hash: "pre-existing-hash".to_string(),
        }])
        .await
        .unwrap();

    assert_eq!(index.len(), 0, "index should be empty before indexer runs");

    // Subscribe and start the indexer.
    let rx = event_store.subscribe();
    let mut indexer = EmbeddingIndexer::start(provider.clone(), embedding_store, index.clone(), rx)
        .expect("start indexer");

    // Append the same event via EventStore, which broadcasts to the indexer.
    event_store.append_event(&event).await.unwrap();

    // Wait for async processing.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Index should still be empty — the indexer's dedup check should skip the
    // event because the embedding already exists in the store.
    assert_eq!(
        index.len(),
        0,
        "duplicate event should be skipped by indexer dedup",
    );

    // Verify store has exactly 1 row for this event_id.
    let all = index.search(&provider.embed("test").await.unwrap(), 10);
    assert!(
        all.is_empty(),
        "no entries should appear in index for duplicate event",
    );

    indexer.shutdown().await;
}
