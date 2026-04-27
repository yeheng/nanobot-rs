# History Recall with Embedding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add embedding-based semantic search across all conversation history, exposed via an automatic hook and an LLM-callable tool.

**Architecture:** New `gasket-embedding` crate owns the full embedding lifecycle (provider abstraction, HNSW index, SQLite persistence, async indexing). Engine integrates via `embedding` feature flag — swapping the keyword-based `HistoryRecallHook` for a semantic one, and adding a `history_search` tool.

**Tech Stack:** Rust, instant-distance (HNSW), sqlx (SQLite BLOB), reqwest (API provider), fastembed (optional ONNX), xxhash-rust (dedup)

**Spec:** `docs/superpowers/specs/2026-04-27-history-recall-embedding-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|----------------|
| `gasket/embedding/Cargo.toml` | Crate definition and dependencies |
| `gasket/embedding/src/lib.rs` | Public API re-exports |
| `gasket/embedding/src/provider.rs` | `EmbeddingProvider` trait + `ApiProvider` + `OnnxProvider` + `ProviderConfig` |
| `gasket/embedding/src/index.rs` | `HnswIndex` — in-memory vector index with soft-delete |
| `gasket/embedding/src/store.rs` | `EmbeddingStore` — SQLite BLOB persistence for embeddings |
| `gasket/embedding/src/indexer.rs` | `EmbeddingIndexer` — async broadcast consumer + cold start rebuild |
| `gasket/embedding/src/searcher.rs` | `RecallSearcher` + `RecallHit` + `RecallConfig` — unified search entry |

### Modified files

| File | Change |
|------|--------|
| `gasket/Cargo.toml` | Add `embedding` to workspace members + workspace deps |
| `gasket/engine/Cargo.toml` | Add `embedding` feature + optional dep on `gasket-embedding` |
| `gasket/storage/src/event_store.rs` | Add `get_events_by_ids_global()` + `get_event_ids_up_to()` |
| `gasket/engine/src/hooks/recall.rs` | Dual cfg-gated struct: semantic (embedding) vs keyword (default) |
| `gasket/engine/src/hooks/mod.rs` | Conditional re-exports based on feature |
| `gasket/engine/src/session/history/builder.rs` | Embedding-aware hook builder when feature enabled |
| `gasket/engine/src/session/compactor.rs` | Add `CompactionListener` support around `delete_events_upto` |
| `gasket/engine/src/config/app_config.rs` | Add `embedding: Option<EmbeddingConfig>` to `Config` |

---

## Task 1: Scaffold the embedding crate

**Files:**
- Create: `gasket/embedding/Cargo.toml`
- Create: `gasket/embedding/src/lib.rs`
- Modify: `gasket/Cargo.toml`

- [ ] **Step 1: Create Cargo.toml for embedding crate**

```toml
[package]
name = "gasket-embedding"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
description = "Embedding-based semantic search for gasket history recall"

[features]
default = []
local-onnx = ["fastembed"]

[dependencies]
tokio.workspace = true
async-trait.workspace = true
anyhow.workspace = true
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "json"] }
instant-distance = "0.5"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
reqwest = { workspace = true }
tiktoken-rs.workspace = true
gasket-types = { path = "../types" }
gasket-storage = { path = "../storage" }
fastembed = { version = "4", optional = true }

[dev-dependencies]
tokio-test = "0.4"
tempfile = "3"
```

- [ ] **Step 2: Create lib.rs with module declarations**

```rust
//! Embedding-based semantic search for gasket history recall.
//!
//! Provides:
//! - `EmbeddingProvider` trait with pluggable backends (API, local ONNX)
//! - `HnswIndex` for in-memory vector search
//! - `EmbeddingStore` for SQLite BLOB persistence
//! - `EmbeddingIndexer` for async indexing pipeline
//! - `RecallSearcher` for unified search entry point

pub mod index;
pub mod indexer;
pub mod provider;
pub mod searcher;
pub mod store;

pub use index::HnswIndex;
pub use indexer::EmbeddingIndexer;
pub use provider::{ApiProvider, EmbeddingProvider, ProviderConfig};
pub use searcher::{RecallConfig, RecallHit, RecallSearcher};
pub use store::{EmbeddingStore, StoredEmbedding};
```

- [ ] **Step 3: Add to workspace**

In `gasket/Cargo.toml`, add `"embedding"` to `workspace.members` array, and add to `[workspace.dependencies]`:

```toml
gasket-embedding = { path = "embedding" }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --package gasket-embedding`
Expected: FAIL — modules not yet created (but Cargo.toml resolves)

- [ ] **Step 5: Create stub modules**

Create minimal `provider.rs`, `index.rs`, `store.rs`, `indexer.rs`, `searcher.rs` each with an empty `pub struct Placeholder;`.

- [ ] **Step 6: Verify compilation**

Run: `cargo check --package gasket-embedding`
Expected: PASS

- [ ] **Step 7: Commit**

```
feat(embedding): scaffold gasket-embedding crate
```

---

## Task 2: Implement EmbeddingProvider trait + ApiProvider

**Files:**
- Create: `gasket/embedding/src/provider.rs` (replace stub)
- Modify: `gasket/embedding/Cargo.toml` (if needed)

- [ ] **Step 1: Write failing test for EmbeddingProvider trait**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider { dim: usize }

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockProvider {
        async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![0.1; self.dim])
        }
        fn dim(&self) -> usize { self.dim }
    }

    #[tokio::test]
    async fn test_mock_provider_embed() {
        let p = MockProvider { dim: 4 };
        let vec = p.embed("hello").await.unwrap();
        assert_eq!(vec.len(), 4);
        assert_eq!(p.dim(), 4);
    }

    #[tokio::test]
    async fn test_provider_config_deserialize_api() {
        let yaml = r#"
type: api
endpoint: "https://api.example.com/v1/embeddings"
model: "text-embedding-3-small"
api_key: "sk-test"
dim: 1536
"#;
        let config: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        match config {
            ProviderConfig::Api { dim, .. } => assert_eq!(dim, 1536),
            _ => panic!("Expected Api variant"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gasket-embedding -- provider`
Expected: FAIL — types not defined

- [ ] **Step 3: Implement EmbeddingProvider trait + ApiProvider + ProviderConfig**

Write the full `provider.rs` with:
- `EmbeddingProvider` trait (`embed`, `embed_batch` with default impl, `dim`)
- `ApiProvider` struct with `reqwest::Client`, endpoint, model, api_key, dim
- `ApiProvider::embed()` — POST to endpoint with Bearer token, parse response
- `ApiProvider::embed_batch()` — single batch HTTP request
- `ProviderConfig` enum (`Api { ... }`, `LocalOnnx { ... }`) with `Deserialize`
- `ProviderConfig::build()` — construct the provider from config
- `OnnxProvider` stub behind `#[cfg(feature = "local-onnx")]`

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-embedding -- provider`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(embedding): add EmbeddingProvider trait and ApiProvider
```

---

## Task 3: Implement HnswIndex

**Files:**
- Create: `gasket/embedding/src/index.rs` (replace stub)

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_search() {
        let index = HnswIndex::new(3);
        index.insert("id1".into(), vec![1.0, 0.0, 0.0]);
        index.insert("id2".into(), vec![0.0, 1.0, 0.0]);
        let results = index.search(&[1.0, 0.0, 0.0], 1);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "id1");
    }

    #[test]
    fn test_remove_tombstone() {
        let index = HnswIndex::new(3);
        index.insert("id1".into(), vec![1.0, 0.0, 0.0]);
        index.remove("id1");
        assert_eq!(index.len(), 0);
        let results = index.search(&[1.0, 0.0, 0.0], 1);
        assert!(results.is_empty() || results[0].0 != "id1");
    }

    #[test]
    fn test_len_excludes_tombstones() {
        let index = HnswIndex::new(3);
        index.insert("a".into(), vec![1.0, 0.0, 0.0]);
        index.insert("b".into(), vec![0.0, 1.0, 0.0]);
        assert_eq!(index.len(), 2);
        index.remove("a");
        assert_eq!(index.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gasket-embedding -- index`
Expected: FAIL

- [ ] **Step 3: Implement HnswIndex**

Write `index.rs` with:
- `HnswIndex` struct wrapping `RwLock<InstantDistance>` + id_map + tombstones + next_id counter
- `new(dim)` constructor
- `insert(event_id, vector)` — allocate u64 key, insert into HNSW + id_map
- `search(vector, k)` — query HNSW, filter tombstones, map u64 → event_id
- `remove(event_id)` — find u64 from reverse lookup, add to tombstones
- `len()` — count minus tombstones

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-embedding -- index`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(embedding): add HnswIndex with insert, search, and soft-delete
```

---

## Task 4: Implement EmbeddingStore (SQLite persistence)

**Files:**
- Create: `gasket/embedding/src/store.rs` (replace stub)

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();
        let store = EmbeddingStore::new(pool.clone());
        store.run_migration().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_save_and_exists() {
        let pool = setup_db().await;
        let store = EmbeddingStore::new(pool);
        assert!(!store.exists("evt-1").await.unwrap());
        store.save("evt-1", "cli:session", "cli", "session",
                   &[0.1, 0.2, 0.3], "user_message", "hash1").await.unwrap();
        assert!(store.exists("evt-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_load_all() {
        let pool = setup_db().await;
        let store = EmbeddingStore::new(pool);
        store.save("evt-1", "cli:a", "cli", "a", &[0.1], "user_message", "h1").await.unwrap();
        store.save("evt-2", "cli:b", "cli", "b", &[0.2], "assistant_message", "h2").await.unwrap();
        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_by_event_ids() {
        let pool = setup_db().await;
        let store = EmbeddingStore::new(pool);
        store.save("evt-1", "cli:a", "cli", "a", &[0.1], "user_message", "h1").await.unwrap();
        store.save("evt-2", "cli:a", "cli", "a", &[0.2], "user_message", "h2").await.unwrap();
        store.delete_by_event_ids(&["evt-1".into()]).await.unwrap();
        assert!(!store.exists("evt-1").await.unwrap());
        assert!(store.exists("evt-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_by_session() {
        let pool = setup_db().await;
        let store = EmbeddingStore::new(pool);
        store.save("evt-1", "cli:a", "cli", "a", &[0.1], "user_message", "h1").await.unwrap();
        store.save("evt-2", "cli:b", "cli", "b", &[0.2], "user_message", "h2").await.unwrap();
        store.delete_by_session("cli:a").await.unwrap();
        assert!(!store.exists("evt-1").await.unwrap());
        assert!(store.exists("evt-2").await.unwrap());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gasket-embedding -- store`
Expected: FAIL

- [ ] **Step 3: Implement EmbeddingStore**

Write `store.rs` with:
- `run_migration()` — CREATE TABLE IF NOT EXISTS `event_embeddings` + indexes
- `save()` — INSERT OR IGNORE (idempotent)
- `save_batch()` — transactional batch insert
- `load_all()` — SELECT all, deserialize BLOB → `Vec<f32>`
- `delete_by_event_ids()` — DELETE WHERE id IN (...)
- `delete_by_session()` — DELETE WHERE session_key = ?
- `exists()` — SELECT 1 WHERE id = ?
- `StoredEmbedding` struct

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-embedding -- store`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(embedding): add EmbeddingStore with SQLite BLOB persistence
```

---

## Task 5: Implement RecallSearcher

**Files:**
- Create: `gasket/embedding/src/searcher.rs` (replace stub)

- [ ] **Step 1: Write failing tests (using MockProvider)****

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::EmbeddingProvider;
    use async_trait::async_trait;

    struct MockProvider { dim: usize }
    #[async_trait]
    impl EmbeddingProvider for MockProvider {
        async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            // Deterministic: different texts get different vectors
            let mut v = vec![0.0; self.dim];
            if text.contains("rust") { v[0] = 1.0; }
            if text.contains("error") { v[1] = 1.0; }
            Ok(v)
        }
        fn dim(&self) -> usize { self.dim }
    }

    #[test]
    fn test_recall_config_defaults() {
        let config = RecallConfig::default();
        assert_eq!(config.top_k, 5);
        assert_eq!(config.token_budget, 500);
        assert!((config.min_score - 0.3).abs() < f32::EPSILON);
    }

    // Integration test with real SQLite + MockProvider would go here
    // to test full recall flow: insert events -> embed -> index -> search -> hits
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gasket-embedding -- searcher`
Expected: FAIL

- [ ] **Step 3: Implement RecallSearcher + RecallHit + RecallConfig**

Write `searcher.rs` with:
- `RecallConfig` with defaults and `Deserialize`
- `RecallHit` struct
- `RecallSearcher` struct with provider, index, store, event_store
- `recall(query, config)` — embed query → search index → filter by min_score → load events → token budget trim
- Note: uses `EventStore::get_events_by_ids_global()` which will be added in Task 7

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-embedding -- searcher`
Expected: PASS (basic tests; full integration tested after Task 7)

- [ ] **Step 5: Commit**

```
feat(embedding): add RecallSearcher with token budget trimming
```

---

## Task 6: Implement EmbeddingIndexer (async pipeline)

**Files:**
- Create: `gasket/embedding/src/indexer.rs` (replace stub)

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // Test that indexer processes broadcast events
    // Test dedup (same event_id not processed twice)
    // Test content filter (short content skipped)
    // Test rebuild_index from store
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gasket-embedding -- indexer`
Expected: FAIL

- [ ] **Step 3: Implement EmbeddingIndexer**

Write `indexer.rs` with:
- `EmbeddingIndexer` struct
- `start()` — spawn background task consuming `broadcast::Receiver<SessionEvent>`
- `process_event()` — filter event type, skip short content, dedup, embed, persist
- `rebuild_index()` — cold start: load all from store → insert into HNSW
- `shutdown()` — set AtomicBool + join handle with 5s timeout
- `COLD_START_BATCH_SIZE = 1000` constant

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-embedding -- indexer`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(embedding): add EmbeddingIndexer with async broadcast pipeline
```

---

## Task 7: Add EventStore methods for cross-session recall

**Files:**
- Modify: `gasket/storage/src/event_store.rs`

- [ ] **Step 1: Write failing test for `get_events_by_ids_global`**

In `gasket/storage/src/event_store.rs`, inside `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn test_get_events_by_ids_global() {
    let pool = setup_test_db().await;
    let store = EventStore::new(pool);

    // Events in different sessions
    let e1 = SessionEvent { session_key: "cli:session1".into(), /* ... */ };
    let e2 = SessionEvent { session_key: "cli:session2".into(), /* ... */ };
    store.append_event(&e1).await.unwrap();
    store.append_event(&e2).await.unwrap();

    let events = store.get_events_by_ids_global(&[e1.id, e2.id]).await.unwrap();
    assert_eq!(events.len(), 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-storage -- test_get_events_by_ids_global`
Expected: FAIL — method not found

- [ ] **Step 3: Implement `get_events_by_ids_global`**

```rust
pub async fn get_events_by_ids_global(&self, ids: &[Uuid]) -> Result<Vec<SessionEvent>, StoreError> {
    if ids.is_empty() { return Ok(vec![]); }
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        "SELECT * FROM session_events WHERE id IN ({}) ORDER BY created_at ASC",
        placeholders
    );
    let mut q = sqlx::query_as::<_, EventRow>(&query);
    for id in ids { q = q.bind(id.to_string()); }
    let rows = q.fetch_all(&self.pool).await?;
    rows.into_iter().map(|r| r.try_into()).collect()
}
```

- [ ] **Step 4: Write failing test for `get_event_ids_up_to`**

```rust
#[tokio::test]
async fn test_get_event_ids_up_to() {
    // Insert events with known sequences, query ids up to a sequence
    let ids = store.get_event_ids_up_to(&session_key, 3).await.unwrap();
    // Assert only events with sequence <= 3 are returned
}
```

- [ ] **Step 5: Implement `get_event_ids_up_to`**

```rust
pub async fn get_event_ids_up_to(&self, session_key: &SessionKey, up_to_seq: i64) -> Result<Vec<String>, StoreError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM session_events WHERE channel = ? AND chat_id = ? AND sequence <= ?"
    )
    .bind(session_key.channel.to_string())
    .bind(&session_key.chat_id)
    .bind(up_to_seq)
    .fetch_all(&self.pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}
```

- [ ] **Step 6: Run all storage tests**

Run: `cargo test --package gasket-storage`
Expected: PASS

- [ ] **Step 7: Commit**

```
feat(storage): add get_events_by_ids_global and get_event_ids_up_to
```

---

## Task 8: Add embedding feature gate to engine crate

**Files:**
- Modify: `gasket/engine/Cargo.toml`

- [ ] **Step 1: Add feature and optional dependency**

In `gasket/engine/Cargo.toml`:
- Add to `[features]`: `embedding = ["gasket-embedding"]`
- Add to `[dependencies]`: `gasket-embedding = { path = "../embedding", optional = true }`

- [ ] **Step 2: Verify compilation without feature**

Run: `cargo check --package gasket-engine`
Expected: PASS (no changes to existing code)

- [ ] **Step 3: Verify compilation with feature**

Run: `cargo check --package gasket-engine --features embedding`
Expected: PASS (gasket-embedding compiles but is not yet used)

- [ ] **Step 4: Commit**

```
feat(engine): add embedding feature gate with optional gasket-embedding dep
```

---

## Task 9: Modify HistoryRecallHook with dual cfg-gated implementation

**Files:**
- Modify: `gasket/engine/src/hooks/recall.rs`

- [ ] **Step 1: Understand current implementation**

Read `gasket/engine/src/hooks/recall.rs` — the entire file contains the keyword-based `HistoryRecallHook`.

- [ ] **Step 2: Wrap existing code in `#[cfg(not(feature = "embedding"))]`**

Wrap the entire current `HistoryRecallHook` struct, impl blocks, and tests in `#[cfg(not(feature = "embedding"))]`.

- [ ] **Step 3: Add embedding-gated implementation**

```rust
#[cfg(feature = "embedding")]
use gasket_embedding::{RecallConfig, RecallSearcher};

#[cfg(feature = "embedding")]
pub struct HistoryRecallHook {
    searcher: Arc<RecallSearcher>,
    config: RecallConfig,
}

#[cfg(feature = "embedding")]
impl HistoryRecallHook {
    pub fn new(searcher: Arc<RecallSearcher>, config: RecallConfig) -> Self {
        Self { searcher, config }
    }
}

#[cfg(feature = "embedding")]
#[async_trait]
impl PipelineHook for HistoryRecallHook {
    fn name(&self) -> &str { "history_recall" }
    fn point(&self) -> HookPoint { HookPoint::AfterHistory }

    async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
        let user_input = match ctx.user_input {
            Some(t) => t,
            None => return Ok(HookAction::Continue),
        };

        let hits = match self.searcher.recall(user_input, &self.config).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("[{}] Recall failed: {}", self.name(), e);
                return Ok(HookAction::Continue);
            }
        };

        if hits.is_empty() { return Ok(HookAction::Continue); }

        let injection = format!(
            "[SYSTEM: Relevant history recalled]\n\n{}",
            hits.iter().map(|h| {
                format!("[{}] (from {}): {}", h.role, h.session_key, h.content)
            }).collect::<Vec<_>>().join("\n---\n")
        );
        ctx.messages.push(ChatMessage::user(injection));

        Ok(HookAction::Continue)
    }
}
```

- [ ] **Step 4: Update `gasket/engine/src/hooks/mod.rs` re-exports**

Ensure `HistoryRecallHook` is re-exported correctly under both cfg configurations. Add conditional imports:

```rust
#[cfg(not(feature = "embedding"))]
pub use recall::HistoryRecallHook;
```

The `#[cfg(feature = "embedding")]` version uses the same name, so a single unconditional `pub use` may suffice if both definitions compile to the same name.

- [ ] **Step 5: Verify both compilation modes**

Run: `cargo check --package gasket-engine` (without feature)
Run: `cargo check --package gasket-engine --features embedding`
Expected: Both PASS

- [ ] **Step 6: Commit**

```
refactor(engine): dual cfg-gate HistoryRecallHook for embedding/keyword modes
```

---

## Task 10: Add history_search tool

**Files:**
- Create: `gasket/engine/src/tools/history_search.rs`
- Modify: `gasket/engine/src/tools/mod.rs` (or builder.rs)

- [ ] **Step 1: Implement history_search tool**

Create `gasket/engine/src/tools/history_search.rs` gated by `#[cfg(feature = "embedding")]`:

```rust
#![cfg(feature = "embedding")]

use async_trait::async_trait;
use gasket_embedding::{RecallConfig, RecallSearcher};
// ... implement the tool trait with:
// name: "history_search"
// parameters: query (required), top_k (optional)
// execute: self.searcher.recall(query, config)
// return formatted recall hits
```

- [ ] **Step 2: Register tool in builder**

In `gasket/engine/src/tools/builder.rs`, add conditional registration:

```rust
#[cfg(feature = "embedding")]
{
    // Register history_search tool with RecallSearcher
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --package gasket-engine --features embedding`
Expected: PASS

- [ ] **Step 4: Commit**

```
feat(engine): add history_search tool behind embedding feature gate
```

---

## Task 11: Add CompactionListener to compactor

**Files:**
- Modify: `gasket/engine/src/session/compactor.rs`

- [ ] **Step 1: Define CompactionListener trait**

```rust
pub trait CompactionListener: Send + Sync {
    fn on_events_deleted<'a>(&'a self, event_ids: &[String]) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
```

Use an async trait pattern (or sync callback with `tokio::spawn`) — simpler: make it sync and let implementers spawn:

```rust
pub trait CompactionListener: Send + Sync {
    fn on_events_deleted(&self, event_ids: &[String]);
}
```

- [ ] **Step 2: Modify `run_compaction` to accept listeners**

Add `listeners: &[Box<dyn CompactionListener>]` parameter to `run_compaction`.

- [ ] **Step 3: Add pre-deletion query + listener notification**

In `run_compaction`, before the existing `delete_events_upto` call (line ~833):

```rust
// Pre-deletion: fetch IDs about to be GC'd
let deleted_ids = event_store.get_event_ids_up_to(session_key, target_sequence).await
    .unwrap_or_default();

// Existing GC
event_store.delete_events_upto(session_key, target_sequence).await?;

// Notify listeners
for listener in listeners {
    listener.on_events_deleted(&deleted_ids);
}
```

- [ ] **Step 4: Thread listeners through the call chain**

`ContextCompactor` struct needs a `Vec<Box<dyn CompactionListener>>` field. Update `spawn_compaction_task` and `try_compact` to pass it through.

- [ ] **Step 5: Verify compilation and existing tests pass**

Run: `cargo test --package gasket-engine`
Expected: PASS (no embedding-specific listeners registered yet)

- [ ] **Step 6: Commit**

```
feat(engine): add CompactionListener trait and integrate into compactor
```

---

## Task 12: Add EmbeddingConfig to app config

**Files:**
- Modify: `gasket/engine/src/config/app_config.rs`

- [ ] **Step 1: Add EmbeddingConfig struct**

```rust
#[cfg(feature = "embedding")]
use gasket_embedding::ProviderConfig;
use gasket_embedding::RecallConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: ProviderConfig,
    #[serde(default)]
    pub recall: RecallConfig,
}
```

Note: `RecallConfig` needs `Serialize`/`Deserialize` derives. Add them in `gasket/embedding/src/searcher.rs` if not already present.

- [ ] **Step 2: Add to Config struct**

```rust
#[serde(default)]
pub embedding: Option<EmbeddingConfig>,
```

Since `EmbeddingConfig` is only available under the `embedding` feature, use cfg:

```rust
#[cfg(feature = "embedding")]
#[serde(default)]
pub embedding: Option<EmbeddingConfig>,
```

- [ ] **Step 3: Verify both compilation modes**

Run: `cargo check --package gasket-engine`
Run: `cargo check --package gasket-engine --features embedding`
Expected: Both PASS

- [ ] **Step 4: Commit**

```
feat(engine): add EmbeddingConfig to app configuration
```

---

## Task 13: Wire up initialization in hook builder

**Files:**
- Modify: `gasket/engine/src/session/history/builder.rs`

- [ ] **Step 1: Add embedding initialization in `build_default_hooks_builder`**

Add a `#[cfg(feature = "embedding")]` block that:
1. Accepts `Option<EmbeddingConfig>` parameter (or reads from config)
2. Builds provider from config
3. Creates EmbeddingStore with shared SqlitePool
4. Creates HnswIndex
5. Runs `EmbeddingIndexer::rebuild_index()` for cold start
6. Creates RecallSearcher
7. Starts EmbeddingIndexer listening to EventStore broadcast
8. Registers `HistoryRecallHook` with searcher
9. Registers `history_search` tool

- [ ] **Step 2: Thread config through from CLI/gateway to builder**

Update the call sites in `gasket/cli/src/commands/agent.rs` and `gasket/cli/src/commands/gateway.rs` to pass `config.embedding` down to the builder.

- [ ] **Step 3: Verify compilation with and without feature**

Run: `cargo check --package gasket-cli`
Run: `cargo check --package gasket-cli --features embedding`
Expected: Both PASS

- [ ] **Step 4: Commit**

```
feat(engine): wire embedding initialization into hook and tool builders
```

---

## Task 14: Integration test — full embedding recall flow

**Files:**
- Add: `gasket/embedding/tests/integration.rs`

- [ ] **Step 1: Write integration test**

Test the full flow:
1. Create in-memory SQLite + EmbeddingStore
2. Create MockProvider
3. Insert several events into store (with embeddings)
4. Rebuild HNSW index
5. Create RecallSearcher
6. Search for a query
7. Verify correct events are returned with proper scores
8. Verify token budget trimming works

- [ ] **Step 2: Write integration test for indexer**

1. Create EventStore + EmbeddingStore + MockProvider
2. Start EmbeddingIndexer with broadcast receiver
3. Append events to EventStore
4. Wait for indexer to process
5. Verify embeddings exist in store + index
6. Shutdown indexer

- [ ] **Step 3: Run integration tests**

Run: `cargo test --package gasket-embedding`
Expected: PASS

- [ ] **Step 4: Commit**

```
test(embedding): add integration tests for full recall flow and indexer
```

---

## Task 15: Update workspace build and verify

**Files:**
- No new files

- [ ] **Step 1: Full workspace build without embedding**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 2: Full workspace build with embedding**

Run: `cargo build --workspace --features embedding`
Expected: PASS

- [ ] **Step 3: Full test suite without embedding**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 4: Full test suite with embedding**

Run: `cargo test --workspace --features embedding`
Expected: PASS

- [ ] **Step 5: Release build**

Run: `cargo build --release --workspace --features embedding`
Expected: PASS

- [ ] **Step 6: Final commit**

```
chore: verify workspace builds with and without embedding feature
```
