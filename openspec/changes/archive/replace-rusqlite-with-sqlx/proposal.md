# Change: Replace rusqlite with sqlx for Native Async SQLite

## Why

All SQLite operations currently use `rusqlite` (synchronous) wrapped in `tokio::task::spawn_blocking` + `Arc<Mutex<Connection>>`. This pattern has 28 `spawn_blocking` call sites across two files (`memory/sqlite.rs` and `agent/task_store_sqlite.rs`). Every database call pays the cost of cloning an `Arc`, acquiring a mutex, and scheduling onto the blocking threadpool — even for trivial single-row queries. Replacing with `sqlx` (native async SQLite via `sqlx::SqlitePool`) eliminates this boilerplate, removes the mutex contention point, and provides connection pooling, compile-time query checking, and built-in migration support.

## What Changes

- **Dependency swap**: Remove `rusqlite` from `nanobot-core/Cargo.toml`, add `sqlx` with `runtime-tokio`, `sqlite` features
- **`SqliteStore` rewrite**: Replace `Arc<Mutex<Connection>>` with `sqlx::SqlitePool`; convert all 24 methods from `spawn_blocking` closures to direct `sqlx::query` / `sqlx::query_as` calls
- **`SqliteTaskStore` rewrite**: Same pattern — replace `Arc<Mutex<Connection>>` with `SqlitePool`; convert 4 async methods + sync helpers
- **Constructor becomes async**: `SqliteStore::new()` and `with_path()` become `async fn` since `SqlitePool::connect()` is async
- **FTS5 compatibility**: sqlx raw queries (`sqlx::query`) support FTS5 `MATCH` and virtual table creation — no behavioral change
- **Schema init via `PRAGMA` + raw SQL**: Keep `execute_batch`-style init using `sqlx::raw_sql()` for multi-statement DDL

## Impact

- Affected specs: `sqlite-async` (new)
- Affected code:
  - `nanobot-core/Cargo.toml` — dependency swap
  - `nanobot-core/src/memory/sqlite.rs` — **full rewrite** of `SqliteStore` internals
  - `nanobot-core/src/agent/task_store_sqlite.rs` — **full rewrite** of `SqliteTaskStore` internals
  - `nanobot-core/src/memory/mod.rs` — re-export unchanged
  - `nanobot-core/src/agent/memory.rs` — `MemoryStore::new()` callers become async
  - `nanobot-core/src/session/manager.rs` — `SessionManager::new()` becomes async
  - `nanobot-core/src/cron/service.rs` — `CronService::new()` becomes async
  - `nanobot-core/src/agent/subagent.rs` — `SubagentManager` init becomes async
  - `nanobot-core/tests/e2e_tests.rs` — test helpers become async
- **Public API change**: `SqliteStore::new()` / `with_path()` become `async fn`. All downstream constructors that create a store must `.await`.
- **No schema change**: All existing SQLite tables, indexes, triggers, and FTS5 virtual tables remain identical.
- **Coordinates with**: `add-history-summarization` (pending) — that change adds `session_summaries` table methods to `SqliteStore`. If landed first, those methods also migrate to sqlx.
