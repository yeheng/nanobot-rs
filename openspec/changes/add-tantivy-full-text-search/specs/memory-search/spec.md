## ADDED Requirements

### Requirement: Tantivy-Powered Memory Search

The system SHALL provide a full-text search tool for long-term memory using the Tantivy search engine.

#### Scenario: Simple keyword search
- **WHEN** the agent calls `memory_tantivy_search` with a query like "project decision"
- **THEN** the system returns memory entries matching those keywords, ranked by relevance score

#### Scenario: Boolean AND search
- **WHEN** the agent calls the tool with `boolean: { must: ["important", "decision"] }`
- **THEN** only memories containing BOTH terms are returned

#### Scenario: Boolean NOT search
- **WHEN** the agent calls the tool with `boolean: { not: ["draft", "template"] }`
- **THEN** memories containing these excluded terms are filtered out

#### Scenario: Fuzzy search with typo tolerance
- **WHEN** the agent calls the tool with `fuzzy: { text: "projet", distance: 1 }`
- **THEN** memories containing "project" are returned despite the typo

#### Scenario: Tag filtering
- **WHEN** the agent calls the tool with `tags: ["lesson", "architecture"]`
- **THEN** only memories with BOTH tags are returned (AND semantics)

#### Scenario: Combined search
- **WHEN** the agent calls the tool with both keyword query and tag filters
- **THEN** results must match the text query AND have all specified tags

#### Scenario: Sort by relevance
- **WHEN** the agent calls the tool with `sort: "relevance"`
- **THEN** results are ordered by BM25 relevance score (highest first)

#### Scenario: Sort by date
- **WHEN** the agent calls the tool with `sort: "date"`
- **THEN** results are ordered by update time (most recent first)

#### Scenario: No results found
- **WHEN** the query matches no memories
- **THEN** the tool returns a message indicating no results were found

---

## ADDED Requirements

### Requirement: Tantivy-Powered History Search

The system SHALL provide a full-text search tool for conversation history using the Tantivy search engine.

#### Scenario: Search by content
- **WHEN** the agent calls `history_tantivy_search` with query "API design discussion"
- **THEN** messages containing those terms are returned from all sessions

#### Scenario: Filter by role
- **WHEN** the agent calls the tool with `role: "user"`
- **THEN** only user messages are searched

#### Scenario: Filter by session
- **WHEN** the agent calls the tool with `session_key: "telegram:12345"`
- **THEN** only messages from that specific session are searched

#### Scenario: Date range search
- **WHEN** the agent calls the tool with `date_range: { from: "2024-01-01", to: "2024-01-31" }`
- **THEN** only messages within that date range are searched

#### Scenario: Combined history search
- **WHEN** the agent uses content query + role filter + session filter together
- **THEN** results match all criteria

---

## ADDED Requirements

### Requirement: Index Synchronization

The system SHALL automatically synchronize Tantivy indexes with SQLite data.

#### Scenario: Memory save triggers index update
- **WHEN** a memory entry is saved via `MemoryStore::save()`
- **THEN** the Tantivy memory index is updated within the same transaction

#### Scenario: Memory delete triggers index update
- **WHEN** a memory entry is deleted via `MemoryStore::delete()`
- **THEN** the corresponding document is removed from the Tantivy index

#### Scenario: History append triggers index update
- **WHEN** a message is appended via `SessionManager::append_message()`
- **THEN** the message is added to the Tantivy history index

#### Scenario: Index failure rollback
- **WHEN** Tantivy index update fails
- **THEN** the SQLite transaction is rolled back to maintain consistency

---

## ADDED Requirements

### Requirement: CLI Index Management

The system SHALL provide CLI commands for index management.

#### Scenario: Rebuild memory index
- **WHEN** the user runs `nanobot search rebuild-memory`
- **THEN** the memory Tantivy index is rebuilt from all SQLite memory entries

#### Scenario: Rebuild history index
- **WHEN** the user runs `nanobot search rebuild-history`
- **THEN** the history Tantivy index is rebuilt from all session messages

#### Scenario: Index rebuild progress
- **WHEN** an index rebuild is in progress
- **THEN** progress is displayed (e.g., "Indexed 150/1000 documents")

---

## MODIFIED Requirements

### Requirement: Agent Tool Registry

The system SHALL provide a tool registry for agent-accessible functions.

**Previous behavior**: Agent has access to `memory_search` tool (SQLite FTS5 only)

**New behavior**: Agent has access to both:
- `memory_search` (SQLite FTS5, basic search)
- `memory_tantivy_search` (Tantivy, advanced search with boolean/fuzzy support)
- `history_tantivy_search` (Tantivy, history search)

#### Scenario: Agent discovers new tools
- **WHEN** the agent loop initializes
- **THEN** all three search tools are registered and available for discovery

#### Scenario: Agent chooses appropriate tool
- **WHEN** the agent needs simple keyword search
- **THEN** it may use either `memory_search` or `memory_tantivy_search`
- **WHEN** the agent needs boolean/fuzzy search
- **THEN** it should use `memory_tantivy_search`
