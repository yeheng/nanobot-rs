# Change: Add MemoryStore trait with SQLite and Vector DB backends

## Why

The current `MemoryStore` trait (`memory/store.rs`) is a simple key-value interface (read/write/delete/append/query-by-prefix). It lacks structured metadata, content-based search, and durable storage beyond flat files. To support long-term agent memory with semantic retrieval, we need a richer storage abstraction that can be backed by SQLite (structured, full-text search) and later by vector databases (semantic/embedding search).

## What Changes

- **Redesign the `MemoryStore` trait** in `nanobot-core::memory` to support:
  - Structured memory entries with metadata (tags, source, timestamps)
  - Content-based search (full-text for SQLite, semantic for vector DB)
  - Filtering by metadata (tags, time range)
- **Add `SqliteStore`** as the first durable backend implementation
  - Uses `rusqlite` for embedded SQLite with FTS5 full-text search
  - SQLite file stored in config directory (`~/.nanobot/memory.db` by default)
  - Gated behind a new `sqlite` feature flag
- **Remove `InMemoryStore`** — no longer needed as a separate implementation
- **Keep existing `FileMemoryStore`** working with the new trait
- **Design the trait to be extensible** for a future vector DB backend (not implemented in this change)

## Impact

- Affected specs: `memory-store` (new capability)
- Affected code:
  - `nanobot-core/src/memory/mod.rs` — updated re-exports, remove `InMemoryStore`
  - `nanobot-core/src/memory/store.rs` — redesigned trait, remove `InMemoryStore`, update `FileMemoryStore`
  - `nanobot-core/src/memory/sqlite.rs` — new file for `SqliteStore` impl
  - `nanobot-core/src/agent/memory.rs` — adapt to new trait
  - `nanobot-core/Cargo.toml` — add `rusqlite` dependency under `sqlite` feature
