## Context

The project uses `rusqlite` (v0.38, `bundled` feature) as the sole database driver. Since `rusqlite` is synchronous, every async method wraps its body in `tokio::task::spawn_blocking` and holds the connection via `Arc<Mutex<Connection>>`. This pattern appears **28 times** across two files:

- `memory/sqlite.rs` (`SqliteStore`) — 24 spawn_blocking calls
- `agent/task_store_sqlite.rs` (`SqliteTaskStore`) — 4 spawn_blocking calls

Each call site follows the same boilerplate:

```rust
let conn = self.conn.clone();
let param = param.to_string();  // clone all params for 'static
tokio::task::spawn_blocking(move || {
    let conn = conn.lock().unwrap();
    conn.execute("...", rusqlite::params![param])?;
    Ok(())
}).await?
```

**Problems with current approach:**

1. **Mutex contention**: All queries serialize through a single `Mutex<Connection>`. Under concurrent load (e.g., multiple channels sending messages), queries block each other even though SQLite WAL mode supports concurrent readers.
2. **Threadpool pressure**: Every query — even trivial `SELECT key FROM kv_store` — occupies a slot in tokio's blocking threadpool.
3. **Boilerplate**: Each method requires ~8 lines of wrapping (clone Arc, clone params, spawn_blocking, lock, execute, unwrap).
4. **No connection pooling**: Single connection means no parallelism for read-heavy workloads.

**Constraints:**

- The project requires `rustls` (no OpenSSL) — sqlx supports this via `runtime-tokio` feature
- FTS5 virtual tables are used for memory search — must verify sqlx raw SQL supports FTS5
- `SqliteTaskStore` uses synchronous `Connection::open()` in `migrate_from_json()` — needs special handling
- `add-history-summarization` (pending change) will add more methods to `SqliteStore` — coordination needed

## Goals / Non-Goals

### Goals

- Replace `rusqlite` with `sqlx` (`runtime-tokio`, `sqlite` features) for native async SQLite
- Eliminate all 28 `spawn_blocking` wrappers
- Replace `Arc<Mutex<Connection>>` with `sqlx::SqlitePool` for connection pooling
- Keep the exact same SQL schema, tables, indexes, triggers, and FTS5 virtual tables
- Maintain identical public API behavior (same method signatures, same return types)

### Non-Goals

- Schema migration tooling (sqlx has built-in migrations, but we keep raw SQL `init_db()` for now)
- Compile-time query checking via `sqlx::query!` macro (requires a live DB at build time; use `sqlx::query` / `sqlx::query_as` instead)
- Changing the database file location or format
- Adding new features (this is a pure refactor)

## Decisions

### Decision 1: Use `sqlx::SqlitePool` instead of single connection

**Why:** `SqlitePool` provides a pool of connections with native async I/O. With WAL mode, multiple readers can run concurrently. The pool handles connection lifecycle, idle timeout, and creation automatically.

**Configuration:**

```rust
let pool = SqlitePool::connect_with(
    SqliteConnectOptions::from_str(&format!("sqlite:{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
).await?;
```

### Decision 2: Use `sqlx::query` (runtime-checked) instead of `sqlx::query!` (compile-time)

**Why:** `sqlx::query!` requires a `DATABASE_URL` env var pointing to a real database at compile time. This adds CI complexity. `sqlx::query` with runtime string SQL is simpler, has identical runtime behavior, and matches the current hand-written SQL approach.

### Decision 3: Constructors become `async fn`

**Why:** `SqlitePool::connect()` is async. This means `SqliteStore::new()`, `SqliteStore::with_path()`, and `SqliteTaskStore::new()` all become `async fn`. All callers are already in async contexts (agent init, session manager init, etc.), so this is a mechanical change.

**Migration pattern:**

```rust
// Before
let store = SqliteStore::new()?;
// After
let store = SqliteStore::new().await?;
```

### Decision 4: Schema initialization via `sqlx::raw_sql()`

**Why:** The current `init_db()` uses `execute_batch()` to run multi-statement DDL (CREATE TABLE, CREATE INDEX, CREATE TRIGGER, CREATE VIRTUAL TABLE). `sqlx::raw_sql()` supports multi-statement execution for SQLite, preserving the existing init pattern without splitting into individual statements.

### Decision 5: FTS5 via raw queries

**Why:** sqlx doesn't have special FTS5 support, but `CREATE VIRTUAL TABLE ... USING fts5(...)` and `SELECT ... WHERE content MATCH ?` work through `sqlx::query()` as plain SQL. The FTS5 triggers also work since they're created in `init_db()`. No behavioral change.

### Decision 6: `SqliteTaskStore.migrate_from_json()` uses synchronous fallback

**Why:** This method runs at startup before the async runtime is fully set up, and uses `Connection::open()` directly. Since it's a one-time migration, keep a small `rusqlite` dependency just for this path — or refactor it to be async. Recommend: make it async and call via the pool, since the tokio runtime is available at that point.

**Alternative considered:** Keep `rusqlite` as an optional dependency for migration only. Rejected — adds dependency complexity for a one-time codepath.

## Risks / Trade-offs

| Risk | Mitigation |
|------|-----------|
| sqlx SQLite doesn't support FTS5 | Verified: FTS5 works through raw SQL queries. Virtual table creation, MATCH queries, and FTS triggers all function via `sqlx::query()`. |
| Constructor change breaks callers | Mechanical: 6 call sites need `.await` added. All are already in async contexts. |
| sqlx adds compile time | sqlx is a large crate. Mitigated by not using `sqlx::query!` macro (no build-time DB connection needed). |
| Concurrent writes with pool | SQLite still serializes writes (even with WAL). Pool helps with concurrent reads, not concurrent writes. But this is no worse than the current mutex approach. |
| `execute_batch` equivalent | `sqlx::raw_sql()` handles multi-statement DDL. Tested with CREATE TABLE + CREATE INDEX + CREATE TRIGGER patterns. |
| Coordination with add-history-summarization | That change adds ~4 methods to SqliteStore. If this lands first, those methods should be written in sqlx style. If that lands first, this change also migrates those methods. |
