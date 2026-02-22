## Context

The nanobot agent framework currently uses a simple key-value `MemoryStore` trait with `FileMemoryStore` (flat files) and `InMemoryStore` (HashMap) implementations. The trait supports `read`, `write`, `delete`, `append`, and `query` (prefix-only). There is no structured metadata, no content-based search, and no durable database backend.

The agent needs richer memory capabilities: storing memories with metadata (tags, source info), searching by content (not just key prefix), and durable storage that survives across sessions without relying on the filesystem layout.

## Goals / Non-Goals

### Goals
- Design a `MemoryStore` trait that supports save and search with structured metadata
- Implement SQLite backend with FTS5 full-text search as first durable backend
- Remove `InMemoryStore`, keep `FileMemoryStore` compatible with new trait
- Make the trait extensible for future vector DB backend (embedding-based semantic search)

### Non-Goals
- Implement vector DB backend in this change (future work)
- Add embedding generation or ML model integration
- Change the agent loop or how it interacts with memory at a high level
- Support multi-tenant or multi-agent memory isolation (can be added later via namespaces)

## Decisions

### Trait Design: Unified trait with optional capabilities

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Save a memory entry. If an entry with the same id exists, it is replaced.
    async fn save(&self, entry: &MemoryEntry) -> anyhow::Result<()>;

    /// Retrieve a memory entry by id.
    async fn get(&self, id: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// Delete a memory entry by id.
    async fn delete(&self, id: &str) -> anyhow::Result<bool>;

    /// Search memories matching a query.
    async fn search(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>>;
}
```

**Rationale**: A single unified trait keeps the interface simple. The `MemoryQuery` struct carries the search parameters, and backends interpret them according to their capabilities (SQLite uses FTS5, vector DB would use embedding similarity). This avoids trait proliferation.

**Alternative considered**: Separate `MemoryReader` / `MemoryWriter` / `MemorySearcher` traits — rejected as over-engineered for the current use case.

### MemoryEntry: Structured with metadata

```rust
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub metadata: MemoryMetadata,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct MemoryMetadata {
    pub source: Option<String>,      // e.g., "user", "agent", "system"
    pub tags: Vec<String>,
    pub extra: serde_json::Value,     // extensible key-value pairs
}
```

**Rationale**: The `extra` field as `serde_json::Value` provides extensibility without schema changes. Tags enable categorical filtering. Source tracks provenance.

### MemoryQuery: Composable filters

```rust
pub struct MemoryQuery {
    pub text: Option<String>,         // full-text / semantic search
    pub tags: Vec<String>,            // filter by tags (AND)
    pub source: Option<String>,       // filter by source
    pub limit: Option<usize>,         // max results
    pub offset: Option<usize>,        // pagination
}
```

**Rationale**: The `text` field is interpreted as FTS5 match by SQLite and as embedding similarity by a future vector backend. This allows the same query interface to work across backends.

### SQLite Backend: `rusqlite` with FTS5

- Use `rusqlite` (synchronous, embedded SQLite) wrapped in `tokio::task::spawn_blocking` for async compatibility
- SQLite database file stored under config directory: `config::config_dir().join("memory.db")` (defaults to `~/.nanobot/memory.db`)
- `SqliteStore::new()` uses the default path; `SqliteStore::with_path(path)` allows custom path (for testing)
- FTS5 virtual table for full-text search on `content`
- Metadata stored as JSON column
- Tags stored in a separate junction table for efficient filtering

**Alternative considered**: `sqlx` (async, compile-time query checks) — rejected because `rusqlite` is simpler for an embedded database, avoids the compile-time macro overhead, and SQLite itself is synchronous anyway.

### Feature gating

- `sqlite` feature flag in `nanobot-core/Cargo.toml` gates `rusqlite` dependency and `SqliteStore`
- Core trait and types are always available (no feature gate)

### Backward compatibility

- The old `MemoryStore` trait is replaced by the new one
- `FileMemoryStore` is updated to implement the new trait
- `InMemoryStore` is removed (no longer needed — tests can use `FileMemoryStore` with a temp dir or `SqliteStore` with a temp file)
- `agent::memory::MemoryStore` wrapper is updated accordingly
- The old `MemoryEntry` (key + updated_at only) is replaced by the richer struct

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| Breaking existing `MemoryStore` consumers | The only consumer is `agent::memory::MemoryStore` wrapper — update it in the same change |
| `rusqlite` adds native dependency (libsqlite3) | Use `rusqlite` with `bundled` feature to compile SQLite from source |
| FTS5 may not be available in all SQLite builds | The `bundled` feature includes FTS5 support |
| `spawn_blocking` overhead for SQLite ops | Acceptable for memory operations which are not on the hot path |

## Open Questions

- Should memory entries support TTL (time-to-live) for automatic expiration? (Defer to future change)
- Should the trait include a `list_tags()` or `count()` method? (Start minimal, add when needed)
