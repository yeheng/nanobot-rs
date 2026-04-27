# History Recall with Embedding - Design Spec

**Date**: 2026-04-27
**Status**: Draft
**Scope**: New `gasket-embedding` crate + engine integration

---

## 1. Overview

History Recall enables the agent to semantically search across all past conversations (all sessions) and inject relevant historical context into the current conversation. It replaces the existing keyword-based `HistoryRecallHook` with an embedding-powered semantic search system.

**Dual-channel design:**
- **Automatic**: `AfterHistory` hook injects relevant history into context on every user message
- **On-demand**: `history_search` tool lets the LLM perform deep search when needed

**Feature-gated**: The entire system is only active when the `embedding` feature flag is enabled on the engine crate.

---

## 2. Architecture

### 2.1 Crate Structure

New workspace member `gasket/embedding/`:

```
gasket/embedding/
├── Cargo.toml
└── src/
    ├── lib.rs          # Public API re-exports
    ├── provider.rs     # EmbeddingProvider trait + ApiProvider + OnnxProvider
    ├── index.rs        # HnswIndex (in-memory vector index)
    ├── store.rs        # EmbeddingStore (SQLite BLOB persistence)
    ├── indexer.rs      # EmbeddingIndexer (async broadcast consumer)
    └── searcher.rs     # RecallSearcher (unified search entry point)
```

### 2.2 Dependency Flow

```
engine (feature="embedding")
  ├── gasket-embedding
  │     ├── gasket-types
  │     ├── gasket-storage (EventStore, SqlitePool)
  │     ├── instant-distance (HNSW)
  │     ├── fastembed (optional, feature="local-onnx")
  │     ├── reqwest (ApiProvider)
  │     └── xxhash-rust (dedup)
  └── (existing deps)
```

---

## 3. EmbeddingProvider Trait

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
}
```

### 3.1 Built-in Implementations

**ApiProvider** — HTTP-based remote embedding API:

```rust
pub struct ApiProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
    dim: usize,
}
```

- Supports any OpenAI-compatible endpoint (OpenAI, DeepSeek, Zhipu, etc.)
- Batch mode via single HTTP request
- 10s request timeout

**OnnxProvider** — Local ONNX inference (optional, feature `local-onnx`):

```rust
pub struct OnnxProvider {
    model: fastembed::TextEmbedding,
    dim: usize,
}
```

- Pure CPU inference, no GPU required
- Default model: `all-MiniLM-L6-v2` (384-dim)

### 3.2 Configuration

```yaml
# ~/.gasket/config.yaml
embedding:
  provider:
    type: api                    # or "local_onnx"
    endpoint: "https://api.openai.com/v1/embeddings"
    model: "text-embedding-3-small"
    api_key: "${OPENAI_API_KEY}"
    dim: 1536
  recall:
    top_k: 5
    token_budget: 500
    min_score: 0.3
```

```rust
pub enum ProviderConfig {
    Api { endpoint: String, model: String, api_key: String, dim: usize },
    LocalOnnx { model: String, dim: usize },
}
```

---

## 4. HNSW Index

Pure Rust in-memory index using `instant-distance`:

```rust
pub struct HnswIndex {
    inner: RwLock<InstantDistance>,
    id_map: RwLock<HashMap<u64, String>>,  // internal u64 -> event_id
    tombstones: RwLock<HashSet<String>>,   // soft-delete set
}
```

| Operation | Description |
|-----------|-------------|
| `insert(event_id, vector)` | Generate incremental u64 key, write to HNSW + id_map |
| `search(vector, k)` | Return `Vec<(event_id, score)>` |
| `remove(event_id)` | Add to tombstone set (HNSW doesn't support physical deletion) |
| `len()` | Current entry count minus tombstones |
| `load_from_store(store)` | Bulk load from SQLite on startup |

---

## 5. SQLite Persistence

### 5.1 Schema

```sql
CREATE TABLE IF NOT EXISTS event_embeddings (
    event_id     TEXT PRIMARY KEY,
    session_key  TEXT NOT NULL,
    channel      TEXT NOT NULL DEFAULT '',
    chat_id      TEXT NOT NULL DEFAULT '',
    embedding    BLOB NOT NULL,
    dim          INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_emb_session ON event_embeddings(session_key);
CREATE INDEX IF NOT EXISTS idx_emb_channel_chat ON event_embeddings(channel, chat_id);
```

- Shares the same SQLite database as `session_events` (same `SqlitePool`)
- `content_hash`: xxhash3-64 of content, used for dedup before embedding generation
- `channel` + `chat_id`: follow the same partitioning pattern as `session_events`

### 5.2 EmbeddingStore

```rust
pub struct EmbeddingStore { pool: SqlitePool }
```

| Method | Description |
|--------|-------------|
| `save(event_id, session_key, embedding, event_type, content_hash)` | Insert one record |
| `save_batch(items)` | Batch insert (within transaction) |
| `load_all()` | Load all for cold-start index rebuild |
| `delete_by_event_ids(ids)` | Clean up after compaction |
| `delete_by_session(session_key)` | Clean up on session clear |
| `exists(event_id)` | Dedup check |

### 5.3 StoredEmbedding

```rust
pub struct StoredEmbedding {
    pub event_id: String,
    pub session_key: String,
    pub embedding: Vec<f32>,
    pub event_type: String,
    pub created_at: String,
}
```

---

## 6. Async Indexing Pipeline

### 6.1 Architecture

The indexer reuses `EventStore`'s existing `broadcast::Sender<SessionEvent>` channel. No new channel needed.

```
EventStore.append_event()
       |
       v
 broadcast::Sender -----> EmbeddingIndexer
                          |
                          | 1. Filter event type (user/assistant only)
                          | 2. Skip short content (< 5 chars)
                          | 3. Dedup check via store.exists()
                          | 4. provider.embed(content)
                          | 5. store.save() + index.insert()
                          v
                         Done
```

### 6.2 EmbeddingIndexer

```rust
pub struct EmbeddingIndexer {
    provider: Arc<dyn EmbeddingProvider>,
    store: EmbeddingStore,
    index: Arc<HnswIndex>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}
```

**Start**: `EmbeddingIndexer::start(provider, store, index, broadcast_rx)`

**Event processing**:
1. Filter: only `UserMessage` and `AssistantMessage`
2. Skip: content shorter than 5 characters
3. Dedup: `store.exists(event_id)` check
4. Generate: `provider.embed(content)`
5. Persist: `store.save(...)` + `index.insert(event_id, embedding)`

**Cold start** (on application boot):
```rust
EmbeddingIndexer::rebuild_index(&store, &index).await?
```
Loads all existing embeddings from SQLite into the in-memory HNSW index.

**Shutdown**: `AtomicBool` flag + timeout on `JoinHandle` (5s).

### 6.3 Error Policy

All errors in the indexing pipeline are logged as warnings and skipped. The indexing pipeline never blocks or crashes the main agent loop.

---

## 7. RecallSearcher

### 7.1 Types

```rust
pub struct RecallSearcher {
    provider: Arc<dyn EmbeddingProvider>,
    index: Arc<HnswIndex>,
    store: EmbeddingStore,
    event_store: Arc<EventStore>,
}

pub struct RecallHit {
    pub event_id: String,
    pub session_key: String,
    pub role: String,
    pub content: String,
    pub score: f32,
    pub created_at: String,
}

pub struct RecallConfig {
    pub top_k: usize,         // default: 5
    pub token_budget: usize,  // default: 500
    pub min_score: f32,       // default: 0.3
}
```

### 7.2 Search Flow

```
query_text
    |
    v
provider.embed(query_text)               -> query_vector
    |
    v
index.search(query_vector, top_k * 2)    -> Vec<(event_id, score)>
    |
    v
filter: score >= min_score               -> candidates
    |
    v
event_store: load full content by IDs    -> events
    |
    v
token_budget trimming                    -> Vec<RecallHit>
```

- **2x over-fetch**: Search returns `top_k * 2` candidates to account for `min_score` filtering
- **Token budget**: Trims results using `tiktoken-rs` counting, stops when budget is exceeded
- **Global search**: No session scoping — searches across all sessions' embeddings

### 7.3 Cross-Session Event Loading

The existing `EventStore.get_events_by_ids()` requires a `SessionKey` parameter and filters by `channel + chat_id`. For global recall, a new method is needed:

```rust
impl EventStore {
    /// Load events by IDs across all sessions (no session scoping).
    pub async fn get_events_by_ids_global(&self, ids: &[Uuid]) -> Result<Vec<SessionEvent>, StoreError> {
        // SELECT * FROM session_events WHERE id IN (?) ORDER BY created_at ASC
    }
}
```

This method will be added to `gasket/storage/src/event_store.rs` as part of the implementation. The `RecallSearcher` uses this method instead of `get_events_by_ids` to enable cross-session result loading.

---

## 8. Engine Integration

### 8.1 Feature Gate

```toml
# gasket/engine/Cargo.toml
[features]
embedding = ["gasket-embedding"]

[dependencies]
gasket-embedding = { path = "../embedding", optional = true }
```

### 8.2 HistoryRecallHook (modified)

When `embedding` feature is enabled, the hook uses `RecallSearcher` instead of keyword matching:

```rust
#[cfg(feature = "embedding")]
pub struct HistoryRecallHook {
    searcher: Arc<RecallSearcher>,
    config: RecallConfig,
}
```

- Runs at `AfterHistory` hook point
- Injects matching hits as a user message with `[SYSTEM: ...]` prefix
- On failure: warn + continue (never blocks the pipeline)

When `embedding` is not enabled, the existing keyword-based implementation is preserved unchanged. **Implementation approach**: Both versions live in the same file (`recall.rs`) as two mutually exclusive struct definitions gated by `#[cfg(feature = "embedding")]` and `#[cfg(not(feature = "embedding"))]`. This is a clean alternate-implementation pattern — same struct name, same trait impl, different internal fields.

### 8.3 history_search Tool (new)

```rust
#[cfg(feature = "embedding")]
pub struct HistorySearchTool {
    searcher: Arc<RecallSearcher>,
    config: RecallConfig,
}
```

- Tool name: `history_search`
- Description: "Search historical conversations across all sessions using semantic similarity"
- Parameters:
  - `query` (string, required): Search text
  - `top_k` (integer, optional): Number of results, default 5
- Returns formatted recall hits for the LLM to reference

### 8.4 Configuration Parsing

The `embedding:` section in `config.yaml` is parsed into the application's existing config struct. Add these types to the engine crate (or a shared config module):

```rust
#[derive(Debug, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: ProviderConfig,
    pub recall: RecallConfig,
}
```

`ProviderConfig` and `RecallConfig` are re-exported from `gasket-embedding`. The config parsing code (where `config.yaml` is deserialized into the runtime config struct) needs to include `pub embedding: Option<EmbeddingConfig>`. When `None`, the embedding system is not initialized.

### 8.5 Initialization

In `build_default_hooks_builder` (or equivalent initialization point):

```rust
#[cfg(feature = "embedding")]
{
    if let Some(ref embed_cfg) = config.embedding {
        let provider = embed_cfg.provider.build()?;
        let store = EmbeddingStore::new(pool.clone());
        let index = Arc::new(HnswIndex::new(embed_cfg.provider.dim()));

        // Cold start: load existing embeddings
        EmbeddingIndexer::rebuild_index(&store, &index).await?;

        let searcher = Arc::new(RecallSearcher::new(
            provider.clone(), index.clone(), store.clone(), event_store.clone(),
        ));

        // Start async indexer (listens to broadcast)
        let rx = event_store.subscribe();
        EmbeddingIndexer::start(provider, store, index, rx).await?;

        // Register hook + tool
        builder = builder.with_hook(Arc::new(HistoryRecallHook::new(
            searcher.clone(), embed_cfg.recall.clone(),
        )));
        // Register history_search tool in tool registry
    }
}
```

---

## 9. Compaction Integration

### 9.1 CompactionListener Trait

```rust
pub trait CompactionListener: Send + Sync {
    fn on_events_deleted(&self, event_ids: &[String]);
}
```

The engine implements this trait with embedding store + index references:

```rust
struct EmbeddingCompactionListener {
    store: EmbeddingStore,
    index: Arc<HnswIndex>,
}
```

On `on_events_deleted`:
1. `store.delete_by_event_ids(ids)` — remove from SQLite
2. `index.remove(id)` for each — add to HNSW tombstone set

### 9.2 Integration Point

### 9.2 Integration Point

The compactor maintains a `Vec<Box<dyn CompactionListener>>`. After `delete_events_upto()`, it calls each listener with the deleted event IDs. This keeps the compactor decoupled from the embedding crate.

**Required change to `EventStore`**: The existing `delete_events_upto()` only returns `u64` (rows affected), not the deleted event IDs. Two options:

1. **Pre-deletion query**: Before calling `delete_events_upto`, query the IDs to be deleted:
   ```rust
   let ids: Vec<String> = event_store.get_event_ids_up_to(session_key, target_sequence).await?;
   event_store.delete_events_upto(session_key, target_sequence).await?;
   for listener in &listeners { listener.on_events_deleted(&ids); }
   ```
2. **Modify return type**: Change `delete_events_upto` to return `Result<Vec<String>, StoreError>` containing the deleted IDs.

Option 1 is preferred — it avoids changing the existing API and keeps the ID-query logic in the compactor where it's needed.

### 9.3 Session Clear

`clear_session` calls `embedding_store.delete_by_session(session_key)` + clears index entries.

---

## 10. Error Handling Summary

| Scenario | Policy |
|----------|--------|
| `provider.embed()` fails | Warn log, skip event, continue |
| `store.save()` fails | Warn log, index already has the entry; rebuild on next restart |
| `index.search()` returns empty | Silent, hook returns Continue |
| Network timeout (ApiProvider) | 10s timeout, warn + skip |
| Cold start large dataset | Batch load (1000 per batch), yield between batches |
| Broadcast overflow | Warn, next compaction rebuild will fill gaps |

**Core principle**: Embedding failure never blocks the main agent loop. Recall is an enhancement, not a critical path.

---

## 11. Testing Strategy

### 11.1 embedding crate (independent)

| Type | Coverage |
|------|----------|
| Unit | `EmbeddingStore` CRUD (in-memory SQLite) |
| Unit | `HnswIndex` insert/search/remove |
| Unit | Content hash dedup logic |
| Unit | `RecallConfig` token_budget trimming |
| Integration | `MockProvider` + full recall flow: insert -> index -> search -> hits |
| Integration | `EmbeddingIndexer` broadcast consumption + store/index consistency |
| Integration | Cold start: store has data -> rebuild_index -> search hits |

```rust
struct MockProvider { dim: usize }

#[async_trait]
impl EmbeddingProvider for MockProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1; self.dim])
    }
    fn dim(&self) -> usize { self.dim }
}
```

### 11.2 engine crate (feature-gated)

```bash
# Run embedding-related tests
cargo test --package gasket-engine --features embedding

# Confirm compilation without embedding
cargo test --package gasket-engine

# Confirm embedding crate standalone
cargo test --package gasket-embedding
```

---

## 12. Performance Estimates

| Operation | Estimated Latency |
|-----------|-------------------|
| Single embedding (API) | 50-200ms (async, non-blocking) |
| Single embedding (local ONNX) | 5-15ms |
| HNSW search (10K entries) | < 1ms |
| HNSW search (100K entries) | < 5ms |
| Cold start load (10K embeddings) | ~2s (SQLite read + HNSW build) |
| Memory (10K x 1536-dim) | ~60MB |
| Memory (10K x 384-dim) | ~15MB |

---

## 13. Migration Path

1. Add `gasket-embedding` crate to workspace members in `gasket/Cargo.toml`:
   - Add `"embedding"` to `workspace.members`
   - Add `gasket-embedding = { path = "embedding" }` to `[workspace.dependencies]`
2. Add `embedding` feature to `gasket-engine/Cargo.toml`
3. Implement embedding crate (provider -> store -> index -> indexer -> searcher)
4. Add `get_events_by_ids_global()` to `EventStore` (for cross-session recall)
5. Add `get_event_ids_up_to()` to `EventStore` (for compaction listener pre-deletion query)
6. Modify `HistoryRecallHook` with `#[cfg(feature = "embedding")]` / `#[cfg(not(...))]`
7. Add `history_search` tool under feature gate
8. Add `CompactionListener` trait and integration to compactor
9. Add `embedding` config section parsing to the runtime config struct
10. Update docs
