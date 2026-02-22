## 1. Core Types and Trait

- [x] 1.1 Define `MemoryEntry` struct with id, content, metadata, created_at, updated_at
- [x] 1.2 Define `MemoryMetadata` struct with source, tags, extra (serde_json::Value)
- [x] 1.3 Define `MemoryQuery` struct with text, tags, source, limit, offset
- [x] 1.4 Define new `MemoryStore` async trait with save/get/delete/search methods

## 2. Update Existing Implementations

- [x] 2.1 Remove `InMemoryStore` and its tests
- [x] 2.2 Update `FileMemoryStore` to implement the new `MemoryStore` trait
- [x] 2.3 Update `agent::memory::MemoryStore` wrapper to work with the new trait
- [x] 2.4 Update `memory/mod.rs` re-exports (remove `InMemoryStore`)

## 3. SQLite Backend

- [x] 3.1 Add `rusqlite` dependency with `bundled` feature under `sqlite` feature flag in Cargo.toml
- [x] 3.2 Implement `SqliteStore::new()` — default path `config_dir()/memory.db`, create/open SQLite file, run migrations (create tables + FTS5 index)
- [x] 3.3 Implement `SqliteStore::with_path(path)` — custom path for testing
- [x] 3.4 Implement `save()` — upsert entry into `memories` table, sync FTS5 index, sync tags
- [x] 3.5 Implement `get()` — query by id, deserialize metadata from JSON column
- [x] 3.6 Implement `delete()` — remove from `memories`, FTS5 index, and tags
- [x] 3.7 Implement `search()` — build dynamic query with FTS5 MATCH for text, JOIN for tags, WHERE for source
- [x] 3.8 Add `sqlite` module gated behind `#[cfg(feature = "sqlite")]`

## 4. Tests

- [x] 4.1 Unit tests for `FileMemoryStore` with new trait (save, get, delete, search with filters)
- [x] 4.2 Unit tests for `SqliteStore` (all CRUD + FTS5 search + tag filtering + file persistence)
- [x] 4.3 Test concurrent access to `SqliteStore`

## 5. Integration

- [x] 5.1 Verify `cargo build` succeeds without `sqlite` feature (no rusqlite dependency)
- [x] 5.2 Verify `cargo build --features sqlite` succeeds
- [x] 5.3 Verify all existing tests pass
