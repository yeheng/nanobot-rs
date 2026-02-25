## Phase 1: Dependency Swap

- [x] 1.1 Remove `rusqlite = { version = "0.38", features = ["bundled"] }` from `nanobot-core/Cargo.toml`
- [x] 1.2 Add `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }` to `nanobot-core/Cargo.toml`
- [x] 1.3 Run `cargo check` to identify all compile errors (expected: every `rusqlite::` reference)

## Phase 2: Rewrite SqliteStore (memory/sqlite.rs)

- [x] 2.1 Replace `Arc<Mutex<Connection>>` with `sqlx::SqlitePool` in `SqliteStore` struct
- [x] 2.2 Convert `SqliteStore::new()` and `with_path()` to `async fn` using `SqlitePool::connect_with()`; configure WAL mode and foreign keys via `SqliteConnectOptions`
- [x] 2.3 Convert `init_db()` to async; use individual `sqlx::query` statements for DDL (tables, indexes, triggers, FTS5 virtual table)
- [x] 2.4 Convert `health_check()` to async; use `sqlx::query_scalar` for `PRAGMA integrity_check` and `sqlx::query` for sentinel insert/delete
- [x] 2.5 Rewrite History API methods (`read_history`, `append_history`, `write_history`, `clear_history`) — remove `spawn_blocking`, use `sqlx::query` directly
- [x] 2.6 Rewrite KV Store API methods (`read_raw`, `write_raw`, `delete_raw`) — remove `spawn_blocking`, use `sqlx::query` directly
- [x] 2.7 Rewrite Session API methods (`save_session_meta`, `load_session_meta`, `append_session_message`, `load_session_messages`, `clear_session_messages`, `update_session_consolidated`, `delete_session`) — remove `spawn_blocking`, use `sqlx::query` / `sqlx::query_as`
- [x] 2.8 Rewrite legacy session methods (`load_session`, `save_session`) — same pattern
- [x] 2.9 Rewrite Cron Jobs API methods (`save_cron_job`, `load_cron_jobs`, `delete_cron_job`) — same pattern
- [x] 2.10 Rewrite `MemoryStore` trait impl (`save`, `get`, `delete`, `search`) — same pattern; handle dynamic SQL in `search_impl` with `sqlx::query` and bind params
- [x] 2.11 Update `row_to_entry()` and `search_impl()` helpers to use `sqlx::Row` instead of `rusqlite::Row`
- [x] 2.12 Update all tests in `mod tests` to use async store construction

## Phase 3: Rewrite SqliteTaskStore (agent/task_store_sqlite.rs)

- [x] 3.1 Replace `Arc<Mutex<Connection>>` with `SqlitePool` in `SqliteTaskStore` struct
- [x] 3.2 Convert `SqliteTaskStore::new()` to `async fn`; init schema via individual `sqlx::query` statements
- [x] 3.3 Rewrite `load_all()` — remove `spawn_blocking`, use `sqlx::query` with manual row mapping
- [x] 3.4 Rewrite `save_task()` — remove `spawn_blocking`, use `sqlx::query` for upsert
- [x] 3.5 Rewrite `remove_tasks()` — remove `spawn_blocking`, use `sqlx::query` in loop
- [x] 3.6 Convert `migrate_from_json()` to `async fn`; use pool for DB access instead of raw `Connection::open()`
- [x] 3.7 Update `row_to_task()` and enum parse helpers to use `sqlx::Row` instead of `rusqlite::Row`
- [x] 3.8 Update all tests to use async store construction

## Phase 4: Migrate Callers

- [x] 4.1 Update `agent/memory.rs`: `MemoryStore::new()` → `async fn`; `with_store()` stays sync (takes existing pool)
- [x] 4.2 Update `session/manager.rs`: `SessionManager::new()` — no change needed (takes `SqliteStore` directly, which is already created async by caller)
- [x] 4.3 Update `cron/service.rs`: `CronService::new()` and `with_store()` → `async fn`; remove `block_in_place`
- [x] 4.4 Update `agent/subagent.rs`: `SubagentManager::new()` → `async fn`; remove `block_in_place`
- [x] 4.5 Update `nanobot-cli/src/main.rs`: add `.await` on `AgentLoop::new()` (2 call sites) and `CronService::new()` (1 call site)
- [x] 4.6 Update `e2e_tests.rs`: add `.await` on `AgentLoop::new()`, `SqliteStore::with_path()` (4 call sites), and `CronService::new()` (1 call site)

## Phase 5: Testing & Validation

- [x] 5.1 Run `cargo test` — all 172 unit + 68 e2e tests pass
- [x] 5.2 Run `cargo clippy` — no warnings
- [x] 5.3 Verify FTS5 search works: `test_sqlite_fts5_search` passes
- [x] 5.4 Verify concurrent access works: `test_sqlite_concurrent_access` passes
- [x] 5.5 Verify session cascade delete works: `test_sqlite_session_delete` passes
- [x] 5.6 Verify task store round-trip: `test_sqlite_store_all_fields_round_trip` passes
- [x] 5.7 Verify no `spawn_blocking` or `rusqlite` references remain in `src/`: only a doc comment mention of `rusqlite` in `search_impl` and unrelated `spawn_blocking` in `shell.rs`
