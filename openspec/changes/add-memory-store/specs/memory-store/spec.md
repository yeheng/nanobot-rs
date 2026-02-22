## ADDED Requirements

### Requirement: Memory Store Trait
The system SHALL provide an async `MemoryStore` trait that defines a unified interface for saving, retrieving, deleting, and searching memory entries. The trait MUST require `Send + Sync` bounds for safe sharing across async tasks.

#### Scenario: Save and retrieve a memory entry
- **WHEN** a memory entry is saved with a given id
- **THEN** the same entry can be retrieved by that id with all fields intact

#### Scenario: Save overwrites existing entry
- **WHEN** a memory entry is saved with an id that already exists
- **THEN** the existing entry is replaced with the new one

#### Scenario: Get non-existent entry
- **WHEN** a get is performed for an id that does not exist
- **THEN** the result is `None`

#### Scenario: Delete existing entry
- **WHEN** a delete is performed for an existing id
- **THEN** the entry is removed and the result is `true`

#### Scenario: Delete non-existent entry
- **WHEN** a delete is performed for a non-existent id
- **THEN** the result is `false`

### Requirement: Structured Memory Entry
The system SHALL represent each memory as a `MemoryEntry` struct containing an id, content, metadata (source, tags, extra JSON), created_at timestamp, and updated_at timestamp. Entry ids MUST be unique within a store instance.

#### Scenario: Entry contains all required fields
- **WHEN** a `MemoryEntry` is created
- **THEN** it MUST have non-empty `id`, `content`, `created_at`, and `updated_at` fields

#### Scenario: Metadata is extensible via extra field
- **WHEN** arbitrary key-value pairs are stored in the `extra` JSON field
- **THEN** they are preserved across save and retrieve operations

### Requirement: Memory Search
The system SHALL support searching memories via a `MemoryQuery` struct that allows filtering by text content, tags, source, with limit and offset for pagination.

#### Scenario: Search by text content
- **WHEN** a search is performed with a `text` query
- **THEN** entries whose content matches the query text are returned

#### Scenario: Filter by tags
- **WHEN** a search is performed with one or more tags
- **THEN** only entries containing all specified tags are returned (AND semantics)

#### Scenario: Filter by source
- **WHEN** a search is performed with a source filter
- **THEN** only entries with the matching source are returned

#### Scenario: Limit and offset
- **WHEN** a search is performed with limit and offset
- **THEN** at most `limit` results are returned, starting from `offset`

#### Scenario: Empty query returns all entries
- **WHEN** a search is performed with no filters set
- **THEN** all entries are returned (subject to limit)

### Requirement: SQLite Store Backend
The system SHALL provide a `SqliteStore` implementation of the `MemoryStore` trait, gated behind the `sqlite` feature flag, that persists memories in a SQLite database file under the config directory (`~/.nanobot/memory.db` by default) with FTS5 full-text search support.

#### Scenario: Default file location
- **WHEN** a `SqliteStore` is created with default settings
- **THEN** the SQLite database file is located at `~/.nanobot/memory.db`

#### Scenario: Persistent storage in file
- **WHEN** entries are saved and the store is closed and reopened
- **THEN** all previously saved entries are still available from the same file

#### Scenario: Full-text search via FTS5
- **WHEN** a text search query is performed
- **THEN** the SQLite backend uses FTS5 to match against entry content

#### Scenario: Concurrent access safety
- **WHEN** multiple async tasks access the same `SqliteStore` instance
- **THEN** all operations complete without data corruption

### Requirement: File Memory Store Compatibility
The existing `FileMemoryStore` SHALL be updated to conform to the new `MemoryStore` trait interface while preserving its current behavior for basic key-value operations. The `InMemoryStore` SHALL be removed.

#### Scenario: FileMemoryStore implements new trait
- **WHEN** `FileMemoryStore` is used through the `MemoryStore` trait
- **THEN** save, get, delete, and search operations work correctly using the filesystem

#### Scenario: InMemoryStore is removed
- **WHEN** the codebase is compiled
- **THEN** `InMemoryStore` no longer exists as a type
