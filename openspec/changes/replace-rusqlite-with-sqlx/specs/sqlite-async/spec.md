## ADDED Requirements

### Requirement: Native Async SQLite via sqlx

The system SHALL use `sqlx::SqlitePool` for all SQLite database operations instead of `rusqlite` with `spawn_blocking` wrappers. All database methods SHALL be natively async without blocking the tokio runtime's async threads.

#### Scenario: Connection pool initialization

- **WHEN** `SqliteStore::new()` or `SqliteStore::with_path()` is called
- **THEN** the system SHALL create a `SqlitePool` with WAL journal mode and foreign keys enabled, and the constructor SHALL be an `async fn`

#### Scenario: Query execution without spawn_blocking

- **WHEN** any database query is executed (read, write, or delete)
- **THEN** the system SHALL use `sqlx::query` or `sqlx::query_as` directly within the async method, without `tokio::task::spawn_blocking` or `std::sync::Mutex`

#### Scenario: Concurrent read access

- **WHEN** multiple async tasks read from the database simultaneously
- **THEN** the connection pool SHALL serve them concurrently via separate connections (enabled by WAL mode), without mutex serialization

### Requirement: Schema Initialization Compatibility

The system SHALL initialize the database schema using the same SQL DDL statements as the current implementation. All tables, indexes, triggers, and FTS5 virtual tables SHALL be created identically.

#### Scenario: FTS5 virtual table creation

- **WHEN** the database is initialized
- **THEN** the system SHALL create the `memories_fts` virtual table using `CREATE VIRTUAL TABLE ... USING fts5(...)` via raw SQL execution

#### Scenario: FTS5 search queries

- **WHEN** a full-text search is performed via `MemoryStore::search()`
- **THEN** the system SHALL execute `SELECT ... WHERE content MATCH ?` queries through `sqlx::query` and return identical results to the current implementation

### Requirement: Identical Public API Behavior

All public methods on `SqliteStore` and `SqliteTaskStore` SHALL maintain the same signatures and return types, with the sole exception that constructors become `async fn`. Callers SHALL observe identical behavior for all CRUD operations.

#### Scenario: SqliteStore method compatibility

- **WHEN** any existing method on `SqliteStore` (e.g., `save`, `get`, `delete`, `search`, `append_session_message`, `load_session_messages`, `save_cron_job`, `load_cron_jobs`) is called
- **THEN** the method SHALL accept the same parameters and return the same result types as the current implementation

#### Scenario: SqliteTaskStore method compatibility

- **WHEN** any existing method on `SqliteTaskStore` (e.g., `load_all`, `save_task`, `remove_tasks`, `migrate_from_json`) is called
- **THEN** the method SHALL accept the same parameters and return the same result types as the current implementation

### Requirement: Async Constructor Migration

All callers of `SqliteStore::new()`, `SqliteStore::with_path()`, and `SqliteTaskStore::new()` SHALL be updated to `.await` the async constructors. This includes `MemoryStore::new()`, `SessionManager::new()`, `CronService::new()`, `SubagentManager` initialization, and test helpers.

#### Scenario: Caller migration

- **WHEN** a component creates a `SqliteStore` or `SqliteTaskStore` instance
- **THEN** the creation call SHALL use `.await` and the caller SHALL be in an async context
