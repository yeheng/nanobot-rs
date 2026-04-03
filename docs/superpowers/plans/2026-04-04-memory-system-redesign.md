# Memory System Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a scenario-based, human-editable memory system for Gasket personal AI assistant with lazy loading, embedding search, and auto-tiering.

**Architecture:** Memory files are standalone `.md` files organized by 6 scenarios (profile/active/knowledge/decisions/episodes/reference) under `~/.gasket/memory/`. SQLite stores embedding vectors for semantic search. A three-phase loading strategy enforces a ~3200 token hard cap. File watcher detects human edits.

**Tech Stack:** Rust 2021, tokio, sqlx (SQLite WAL), serde_yaml, fastembed (feature-gated), tiktoken-rs, notify crate

**Spec:** `docs/superpowers/specs/2026-04-04-memory-system-redesign-design.md`

---

## File Structure

### New Files (in `gasket/storage/src/`)

| File | Responsibility |
|------|---------------|
| `memory/mod.rs` | Module root, public re-exports |
| `memory/types.rs` | Core enums and structs: Scenario, Frequency, MemoryMeta, MemoryFile, MemoryQuery, MemoryHit |
| `memory/frontmatter.rs` | YAML frontmatter parsing and serialization |
| `memory/path.rs` | Path resolution helpers: base dir, scenario dir, memory file paths |
| `memory/store.rs` | `MemoryStore` trait and `FileMemoryStore` implementation (CRUD) |
| `memory/index.rs` | `IndexManager` — _INDEX.md generation and parsing |
| `memory/retrieval.rs` | `RetrievalEngine` — tag search, embedding search, merged ranking |
| `memory/lifecycle.rs` | Frequency decay/promotion, access log, batch flush |
| `memory/watcher.rs` | File watcher with debouncing (notify crate) |
| `memory/dedup.rs` | Cross-session deduplication cron task |

### New Files (in `gasket/engine/src/`)

| File | Responsibility |
|------|---------------|
| `agent/memory_manager.rs` | `MemoryManager` facade: three-phase loading, scenario detection |

### Modified Files

| File | Changes |
|------|---------|
| `gasket/storage/Cargo.toml` | Add dependencies: serde_yaml, tiktoken-rs, notify; feature gate |
| `gasket/storage/src/lib.rs` | Add `pub mod memory;` |
| `gasket/engine/Cargo.toml` | Add dependency on notify (if needed at engine level) |
| `gasket/engine/src/agent/mod.rs` | Add `pub mod memory_manager;` |
| `gasket/engine/src/agent/loop_.rs` | Inject memory loading into agent pipeline |

### Database Changes

| Table | Purpose |
|-------|---------|
| `memory_embeddings` | Embedding vectors + metadata for each memory file |
| `dedup_reports` | Pending dedup suggestions for agent review |

---

## Phase 1: Core Types & Path Resolution

### Task 1: Define Core Enums and Structs

**Files:**
- Create: `gasket/storage/src/memory/mod.rs`
- Create: `gasket/storage/src/memory/types.rs`
- Test: `gasket/storage/src/memory/types.rs` (inline tests)

- [ ] **Step 1: Create module structure and types**

```rust
// gasket/storage/src/memory/mod.rs
mod types;
mod frontmatter;
mod path;
mod store;
mod index;
mod retrieval;
mod lifecycle;

pub use types::*;
pub use frontmatter::{parse_frontmatter, serialize_frontmatter, FrontmatterData};
pub use path::{memory_base_dir, scenario_dir, memory_file_path};
pub use store::{MemoryStore, FileMemoryStore};
pub use index::{IndexManager, FileIndexManager, MemoryIndexEntry};
pub use retrieval::{RetrievalEngine, SearchResult};
pub use lifecycle::{AccessLog, FrequencyManager};
```

```rust
// gasket/storage/src/memory/types.rs
use serde::{Deserialize, Serialize};
use std::fmt;

/// Memory scenario — the primary organizational dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scenario {
    Profile,
    Active,
    Knowledge,
    Decisions,
    Episodes,
    Reference,
}

impl Scenario {
    pub fn dir_name(&self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Active => "active",
            Self::Knowledge => "knowledge",
            Self::Decisions => "decisions",
            Self::Episodes => "episodes",
            Self::Reference => "reference",
        }
    }

    pub fn all() -> &'static [Scenario] {
        &[
            Scenario::Profile,
            Scenario::Active,
            Scenario::Knowledge,
            Scenario::Decisions,
            Scenario::Episodes,
            Scenario::Reference,
        ]
    }

    /// Profile never decays; Active is always hot.
    pub fn is_exempt_from_decay(&self) -> bool {
        matches!(self, Self::Profile)
    }

    pub fn from_dir_name(name: &str) -> Option<Self> {
        match name {
            "profile" => Some(Self::Profile),
            "active" => Some(Self::Active),
            "knowledge" => Some(Self::Knowledge),
            "decisions" => Some(Self::Decisions),
            "episodes" => Some(Self::Episodes),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.dir_name())
    }
}

/// Access frequency tier — controls loading priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Hot,
    Warm,
    Cold,
    Archived,
}

impl Frequency {
    /// Decay ordering: Hot > Warm > Cold > Archived
    pub fn rank(&self) -> u8 {
        match self {
            Self::Hot => 0,
            Self::Warm => 1,
            Self::Cold => 2,
            Self::Archived => 3,
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "hot" => Self::Hot,
            "warm" => Self::Warm,
            "cold" => Self::Cold,
            "archived" => Self::Archived,
            _ => Self::Warm,
        }
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
            Self::Archived => write!(f, "archived"),
        }
    }
}

/// Parsed memory metadata from YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMeta {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub memory_type: String,
    pub scenario: Scenario,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_frequency")]
    pub frequency: Frequency,
    #[serde(default)]
    pub access_count: u32,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub last_accessed: Option<String>,
    #[serde(default)]
    pub auto_expire: bool,
    #[serde(default)]
    pub expires: Option<String>,
    pub tokens: u32,
    #[serde(default)]
    pub superseded_by: Option<String>,
}

fn default_frequency() -> Frequency {
    Frequency::Warm
}

/// A complete memory file (frontmatter + content).
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub meta: MemoryMeta,
    pub content: String,
}

/// Query for memory retrieval.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    pub text: Option<String>,
    pub tags: Vec<String>,
    pub scenario: Option<Scenario>,
    pub max_tokens: usize,
}

/// A single search result with scoring info.
#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub path: String,
    pub scenario: Scenario,
    pub title: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub score: f32,
    pub tokens: usize,
}

/// Token budget configuration.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub bootstrap: usize,   // default: 700
    pub scenario: usize,    // default: 1500
    pub on_demand: usize,   // default: 1000
    pub total_cap: usize,   // default: 3200
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            bootstrap: 700,
            scenario: 1500,
            on_demand: 1000,
            total_cap: 3200,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_roundtrip() {
        for s in Scenario::all() {
            assert_eq!(Scenario::from_dir_name(s.dir_name()), Some(*s));
        }
    }

    #[test]
    fn frequency_ordering() {
        assert!(Frequency::Hot.rank() < Frequency::Warm.rank());
        assert!(Frequency::Warm.rank() < Frequency::Cold.rank());
        assert!(Frequency::Cold.rank() < Frequency::Archived.rank());
    }

    #[test]
    fn scenario_exempt_from_decay() {
        assert!(Scenario::Profile.is_exempt_from_decay());
        assert!(!Scenario::Knowledge.is_exempt_from_decay());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --package gasket-storage memory::types --no-fail-fast`
Expected: All 3 tests PASS

- [ ] **Step 3: Commit**

```bash
git add gasket/storage/src/memory/
git commit -m "feat(storage): add memory system core types — Scenario, Frequency, MemoryMeta"
```

---

### Task 2: Path Resolution Helpers

**Files:**
- Create: `gasket/storage/src/memory/path.rs`
- Test: inline

- [ ] **Step 1: Write path resolution module**

```rust
// gasket/storage/src/memory/path.rs
use super::types::Scenario;
use std::path::PathBuf;

/// Base directory for all memory files: `~/.gasket/memory/`
pub fn memory_base_dir() -> PathBuf {
    super::super::config_dir().join("memory")
}

/// Directory for a specific scenario.
pub fn scenario_dir(scenario: Scenario) -> PathBuf {
    memory_base_dir().join(scenario.dir_name())
}

/// Full path to a memory file within a scenario.
pub fn memory_file_path(scenario: Scenario, filename: &str) -> PathBuf {
    scenario_dir(scenario).join(filename)
}

/// Path to the _INDEX.md for a scenario.
pub fn index_path(scenario: Scenario) -> PathBuf {
    scenario_dir(scenario).join("_INDEX.md")
}

/// History directory for a scenario.
pub fn history_dir(scenario: Scenario) -> PathBuf {
    memory_base_dir().join(".history").join(scenario.dir_name())
}

/// History file path for a specific version.
pub fn history_file_path(scenario: Scenario, filename: &str, timestamp: &str) -> PathBuf {
    let stem = filename.trim_end_matches(".md");
    history_dir(scenario).join(format!("{}.{}.md", stem, timestamp))
}

/// List all memory .md files in a scenario directory (excluding _INDEX.md).
pub async fn list_memory_files(scenario: Scenario) -> std::io::Result<Vec<String>> {
    let dir = scenario_dir(scenario);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(&dir).await?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".md") && name != "_INDEX.md" && !name.starts_with('.') {
            files.push(name);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_base_dir_is_under_gasket() {
        let base = memory_base_dir();
        assert!(base.to_string_lossy().contains(".gasket"));
        assert!(base.to_string_lossy().ends_with("memory"));
    }

    #[test]
    fn scenario_dir_uses_correct_name() {
        assert_eq!(
            scenario_dir(Scenario::Knowledge).file_name().unwrap(),
            "knowledge"
        );
        assert_eq!(
            scenario_dir(Scenario::Profile).file_name().unwrap(),
            "profile"
        );
    }

    #[test]
    fn index_path_is_index_md() {
        let p = index_path(Scenario::Decisions);
        assert!(p.to_string_lossy().ends_with("decisions/_INDEX.md"));
    }

    #[test]
    fn history_file_path_format() {
        let p = history_file_path(
            Scenario::Knowledge,
            "rust-async.md",
            "2026-04-03T10:00:00",
        );
        let s = p.to_string_lossy();
        assert!(s.contains(".history"));
        assert!(s.contains("rust-async.2026-04-03T10:00:00.md"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --package gasket-storage memory::path --no-fail-fast`
Expected: All 4 tests PASS

- [ ] **Step 3: Commit**

```bash
git add gasket/storage/src/memory/path.rs
git commit -m "feat(storage): add memory path resolution helpers"
```

---

### Task 3: Frontmatter Parsing & Serialization

**Files:**
- Create: `gasket/storage/src/memory/frontmatter.rs`
- Test: inline

- [ ] **Step 1: Write frontmatter parser and serializer**

```rust
// gasket/storage/src/memory/frontmatter.rs
use super::types::MemoryMeta;
use anyhow::{Context, Result};

/// Parse YAML frontmatter from a .md file content.
/// Expects content to start with `---\n`.
pub fn parse_frontmatter(content: &str) -> Result<MemoryMeta> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        anyhow::bail!("Memory file does not start with YAML frontmatter delimiter");
    }

    let end = content[3..]
        .find("\n---")
        .context("No closing frontmatter delimiter found")?;

    let yaml_str = &content[3..end + 3];
    let meta: MemoryMeta = serde_yaml::from_str(yaml_str)
        .context("Failed to parse YAML frontmatter")?;

    Ok(meta)
}

/// Extract the body content (everything after the closing `---`).
pub fn extract_body(content: &str) -> &str {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content;
    }
    if let Some(end) = content[3..].find("\n---") {
        let body_start = end + 7; // skip opening `---\n` + closing `\n---\n`
        if body_start < content.len() {
            return content[body_start..].trim();
        }
    }
    ""
}

/// Serialize metadata back to a full .md file with frontmatter.
pub fn serialize_memory_file(meta: &MemoryMeta, body: &str) -> String {
    let yaml = serde_yaml::to_string(meta).unwrap_or_default();
    format!("---\n{}---\n\n{}", yaml.trim_end(), body.trim())
}

/// Parse a complete memory file (frontmatter + body).
pub fn parse_memory_file(content: &str) -> Result<(MemoryMeta, String)> {
    let meta = parse_frontmatter(content)?;
    let body = extract_body(content).to_string();
    Ok((meta, body))
}

/// Count approximate tokens using a simple heuristic.
/// For accurate counting, use tiktoken-rs when available.
pub fn estimate_tokens(text: &str) -> u32 {
    // Rough approximation: ~4 characters per token for English/mixed content
    (text.len() as u32) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{Frequency, Scenario};

    fn sample_md() -> &'static str {
        r#"---
id: mem_test123
title: "Test memory"
type: concept
scenario: knowledge
tags: [rust, async]
frequency: warm
access_count: 5
created: "2026-04-01T10:00:00Z"
updated: "2026-04-03T15:30:00Z"
last_accessed: "2026-04-03T15:30:00Z"
auto_expire: false
expires: null
tokens: 100
---

This is the body content about Rust async patterns.
"#
    }

    #[test]
    fn parse_valid_frontmatter() {
        let meta = parse_frontmatter(sample_md()).unwrap();
        assert_eq!(meta.id, "mem_test123");
        assert_eq!(meta.title, "Test memory");
        assert_eq!(meta.scenario, Scenario::Knowledge);
        assert_eq!(meta.tags, vec!["rust", "async"]);
        assert_eq!(meta.frequency, Frequency::Warm);
        assert_eq!(meta.access_count, 5);
    }

    #[test]
    fn extract_body_content() {
        let body = extract_body(sample_md());
        assert!(body.contains("Rust async patterns"));
        assert!(!body.contains("---"));
    }

    #[test]
    fn parse_memory_file_returns_both() {
        let (meta, body) = parse_memory_file(sample_md()).unwrap();
        assert_eq!(meta.id, "mem_test123");
        assert!(body.contains("body content"));
    }

    #[test]
    fn serialize_roundtrip() {
        let (meta, body) = parse_memory_file(sample_md()).unwrap();
        let serialized = serialize_memory_file(&meta, &body);
        let (meta2, body2) = parse_memory_file(&serialized).unwrap();
        assert_eq!(meta.id, meta2.id);
        assert_eq!(meta.title, meta2.title);
        assert_eq!(body.trim(), body2.trim());
    }

    #[test]
    fn parse_missing_delimiter_fails() {
        let content = "Just some text without frontmatter";
        assert!(parse_frontmatter(content).is_err());
    }

    #[test]
    fn estimate_tokens_reasonable() {
        let tokens = estimate_tokens("Hello world, this is a test.");
        // ~30 chars / 4 ≈ 7 tokens
        assert!(tokens > 0 && tokens < 20);
    }
}
```

- [ ] **Step 2: Update `mod.rs` to include frontmatter**

Ensure `gasket/storage/src/memory/mod.rs` has `mod frontmatter;` and the re-export.

- [ ] **Step 3: Run tests**

Run: `cargo test --package gasket-storage memory::frontmatter --no-fail-fast`
Expected: All 6 tests PASS

- [ ] **Step 4: Commit**

```bash
git add gasket/storage/src/memory/
git commit -m "feat(storage): add frontmatter parsing and serialization"
```

---

### Task 4: Update Cargo.toml and Wire Module

**Files:**
- Modify: `gasket/storage/Cargo.toml`
- Modify: `gasket/storage/src/lib.rs`

- [ ] **Step 1: Add dependencies to storage Cargo.toml**

Add to `gasket/storage/Cargo.toml` under `[dependencies]`:
```toml
serde_yaml = "0.9"
```

Note: `tiktoken-rs` and `notify` will be added in later phases when needed.

- [ ] **Step 2: Add `pub mod memory;` to `gasket/storage/src/lib.rs`**

Add at the end of the module declarations:
```rust
pub mod memory;
```

- [ ] **Step 3: Run full storage tests**

Run: `cargo test --package gasket-storage`
Expected: All existing + new tests PASS

- [ ] **Step 4: Commit**

```bash
git add gasket/storage/Cargo.toml gasket/storage/src/lib.rs
git commit -m "feat(storage): wire memory module into storage crate"
```

---

## Phase 2: File System CRUD + Version History

### Task 5: MemoryStore Trait and FileMemoryStore

**Files:**
- Create: `gasket/storage/src/memory/store.rs`
- Test: inline + integration

- [ ] **Step 1: Write the MemoryStore trait and FileMemoryStore implementation**

The implementation should cover:
- `init()` — create `~/.gasket/memory/` directory structure with all 6 scenario dirs
- `create()` — write new .md file with auto-generated frontmatter
- `read()` — parse .md file, return MemoryFile
- `update()` — save version history, rewrite file
- `delete()` — remove file, clean up SQLite entry
- `list()` — list all files in a scenario

Key design:
- Use `tokio::fs` for all file operations
- Generate UUIDv7 for `id` field using `uuid::Uuid::now_v7()`
- On `update()`, copy current file to `.history/` before overwriting
- Prune history to max 10 versions per file

Full implementation should be ~200-300 lines. Follow existing `SqliteStore` patterns for error handling (`anyhow::Result`).

- [ ] **Step 2: Write tests for init, create, read**

```rust
#[tokio::test]
async fn test_init_creates_directory_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileMemoryStore::new(tmp.path().to_path_buf());
    store.init().await.unwrap();

    for scenario in Scenario::all() {
        let dir = tmp.path().join(scenario.dir_name());
        assert!(dir.exists(), "Missing dir: {:?}", dir);
    }
}

#[tokio::test]
async fn test_create_and_read() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileMemoryStore::new(tmp.path().to_path_buf());
    store.init().await.unwrap();

    let id = store.create(Scenario::Knowledge, "Test title", "concept", &["rust"]).await.unwrap();
    let mem = store.read(Scenario::Knowledge, &format!("{}.md", id)).await.unwrap();
    assert_eq!(mem.meta.title, "Test title");
    assert_eq!(mem.meta.scenario, Scenario::Knowledge);
}
```

- [ ] **Step 3: Write tests for update with history**

```rust
#[tokio::test]
async fn test_update_preserves_history() {
    // Create → update → verify .history/ has previous version
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-storage memory::store --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/src/memory/store.rs
git commit -m "feat(storage): implement MemoryStore CRUD with version history"
```

---

## Phase 3: Index Management

### Task 6: IndexManager — _INDEX.md Generation and Parsing

**Files:**
- Create: `gasket/storage/src/memory/index.rs`
- Test: inline

- [ ] **Step 1: Implement IndexManager**

Key functions:
- `regenerate(scenario)` — scan all .md files in scenario, parse frontmatter, generate table
- `read_index(scenario)` — parse existing _INDEX.md into structured entries
- Atomic write: write to `.tmp`, fsync, rename

Index entry struct:
```rust
pub struct MemoryIndexEntry {
    pub id: String,
    pub title: String,
    pub memory_type: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub tokens: u32,
    pub filename: String,
    pub updated: String,
}
```

Regeneration must preserve the `<!-- HUMAN_NOTES_START -->` ... `<!-- HUMAN_NOTES_END -->` section.

- [ ] **Step 2: Write tests**

Test cases:
- Empty scenario generates valid index
- Adding a memory and regenerating includes it in the index
- Human notes are preserved across regenerations
- Atomic write (verify .tmp is not left behind on success)

- [ ] **Step 3: Run tests**

Run: `cargo test --package gasket-storage memory::index --no-fail-fast`

- [ ] **Step 4: Commit**

```bash
git add gasket/storage/src/memory/index.rs
git commit -m "feat(storage): implement IndexManager with atomic writes"
```

---

## Phase 4: SQLite Embedding Store

### Task 7: Database Schema Migration

**Files:**
- Modify: `gasket/storage/src/lib.rs` (add migration)
- Create: `gasket/storage/src/memory/embedding_store.rs`

- [ ] **Step 1: Add migration for memory_embeddings table**

In `SqliteStore::with_path()` or equivalent initialization, add:
```sql
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_path   TEXT PRIMARY KEY,
    scenario      TEXT NOT NULL,
    tags          TEXT,
    frequency     TEXT NOT NULL DEFAULT 'warm',
    embedding     BLOB NOT NULL,
    token_count   INTEGER NOT NULL,
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_mem_emb_scenario ON memory_embeddings(scenario);
CREATE INDEX IF NOT EXISTS idx_mem_emb_frequency ON memory_embeddings(frequency);

CREATE TABLE IF NOT EXISTS dedup_reports (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_a      TEXT NOT NULL,
    memory_b      TEXT NOT NULL,
    similarity    REAL NOT NULL,
    suggestion    TEXT NOT NULL,
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved      BOOLEAN DEFAULT FALSE
);
```

- [ ] **Step 2: Implement EmbeddingStore**

Functions:
- `upsert_embedding(path, scenario, tags, frequency, embedding_bytes, token_count)`
- `delete_embedding(path)`
- `search_by_tags(tags, limit)` — JSON tag array matching via SQL
- `search_by_embedding(query_vector, limit)` — fetch top-50 vectors, compute cosine in Rust
- `get_all_embeddings(scenario)` — for dedup scan

Follow existing `session_embeddings` patterns from `gasket/storage/src/lib.rs` for BLOB handling with `bytemuck`.

- [ ] **Step 3: Write tests**

Test cases:
- Insert and retrieve embedding
- Search by embedding returns correct ordering
- Search by tags matches correctly
- Delete removes entry

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-storage memory::embedding_store --no-fail-fast`

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/src/
git commit -m "feat(storage): add memory_embeddings table and EmbeddingStore"
```

---

## Phase 5: Retrieval Engine

### Task 8: Combined Search with Normalized Scoring

**Files:**
- Create: `gasket/storage/src/memory/retrieval.rs`
- Test: inline

- [ ] **Step 1: Implement RetrievalEngine**

Key algorithm from spec section 7.5:
```rust
pub struct RetrievalEngine {
    store: FileMemoryStore,
    // embedding_store: EmbeddingStore, // added in task 7
}

impl RetrievalEngine {
    /// Tag search: read _INDEX.md, filter by tag intersection
    pub async fn search_by_tags(&self, tags: &[String], scenario: Option<Scenario>, limit: usize) -> Result<Vec<MemoryHit>>;

    /// Embedding search: cosine similarity via SQLite
    pub async fn search_by_embedding(&self, query: &str, limit: usize) -> Result<Vec<MemoryHit>>;

    /// Combined search with normalized scoring
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>>;
}
```

Normalized merge (spec section 7.5):
```
TAG_WEIGHT = 0.4, EMBEDDING_WEIGHT = 0.6
tag_score = matching_tags / total_query_tags  → [0.0, 1.0]
emb_score = (cosine + 1.0) / 2.0             → [0.0, 1.0]
merged = tag_score * 0.4 + emb_score * 0.6
```

- [ ] **Step 2: Write tests for scoring**

Test the merge algorithm with known inputs to verify normalization is correct.

- [ ] **Step 3: Run tests**

Run: `cargo test --package gasket-storage memory::retrieval --no-fail-fast`

- [ ] **Step 4: Commit**

```bash
git add gasket/storage/src/memory/retrieval.rs
git commit -m "feat(storage): implement RetrievalEngine with normalized merge scoring"
```

---

### Task 9: Three-Phase Loading with Token Budget

**Files:**
- Create: `gasket/engine/src/agent/memory_manager.rs`
- Test: inline

- [ ] **Step 1: Implement MemoryManager facade**

```rust
pub struct MemoryManager {
    store: FileMemoryStore,
    index_manager: FileIndexManager,
    retrieval: RetrievalEngine,
    access_log: AccessLog,
    budget: TokenBudget,
}

impl MemoryManager {
    /// Phase 1: Always load profile + active (~700 tokens)
    async fn load_bootstrap(&self) -> Result<Vec<MemoryFile>>;

    /// Phase 2: Load scenario-specific hot/warm items (~1500 tokens)
    async fn load_scenario(&self, scenario: Scenario, tags: &[String]) -> Result<Vec<MemoryFile>>;

    /// Phase 3: On-demand search (~1000 tokens)
    async fn load_on_demand(&self, query: &MemoryQuery) -> Result<Vec<MemoryFile>>;

    /// Combined three-phase loading respecting total budget
    pub async fn load_for_context(&self, query: &MemoryQuery) -> Result<Vec<MemoryFile>>;
}
```

Budget enforcement follows spec section 10.2. Log access in-memory via `AccessLog`.

- [ ] **Step 2: Write tests**

Test with temporary directories and known memory files:
- Bootstrap loads profile + active within budget
- Scenario phase respects remaining budget
- On-demand phase fills remaining capacity
- Total never exceeds hard cap

- [ ] **Step 3: Run tests**

Run: `cargo test --package gasket-engine memory_manager --no-fail-fast`

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/agent/memory_manager.rs
git commit -m "feat(engine): implement MemoryManager with three-phase loading"
```

---

## Phase 6: Frequency Lifecycle

### Task 10: Access Log and Batched Flush

**Files:**
- Create: `gasket/storage/src/memory/lifecycle.rs`
- Test: inline

- [ ] **Step 1: Implement AccessLog and FrequencyManager**

```rust
/// In-memory access log for deferred batched writes.
pub struct AccessLog {
    entries: Vec<(String, chrono::DateTime<chrono::Utc>)>, // (path, timestamp)
    flush_threshold: usize, // default: 50
    last_flush: chrono::DateTime<chrono::Utc>,
}

impl AccessLog {
    pub fn record(&mut self, path: &str);
    pub fn should_flush(&self) -> bool;
    pub fn drain(&mut self) -> Vec<(String, chrono::DateTime<chrono::Utc>)>;
}

/// Frequency decay and promotion logic.
pub struct FrequencyManager;

impl FrequencyManager {
    /// Recalculate frequency based on decay rules (spec 8.1)
    pub fn recalculate(current: Frequency, last_accessed: &str) -> Frequency;

    /// Batch decay job (spec 8.1.1) — scan all memories, update tiers
    pub async fn run_decay_batch(store: &FileMemoryStore) -> Result<()>;

    /// Batch flush access log to disk (spec 8.3)
    pub async fn flush_access_log(
        log: &mut AccessLog,
        store: &FileMemoryStore,
        index_mgr: &FileIndexManager,
    ) -> Result<()>;
}
```

Decay rules (spec 8.1):
- hot → warm: 7 days
- warm → cold: 30 days
- cold → archived: 90 days
- Profile exempt

Promotion rules (spec 8.2):
- cold → warm: on access
- warm → hot: 3+ accesses in 7 days

- [ ] **Step 2: Write tests**

Test cases:
- `recalculate` returns correct tier based on time delta
- Profile exempt from decay
- Access log batches correctly
- `drain` empties the log

- [ ] **Step 3: Run tests**

Run: `cargo test --package gasket-storage memory::lifecycle --no-fail-fast`

- [ ] **Step 4: Commit**

```bash
git add gasket/storage/src/memory/lifecycle.rs
git commit -m "feat(storage): implement frequency lifecycle with batched access tracking"
```

---

## Phase 7: File Watcher

### Task 11: File Watcher with Debouncing

**Files:**
- Create: `gasket/storage/src/memory/watcher.rs`
- Modify: `gasket/storage/Cargo.toml` (add `notify` dependency)

- [ ] **Step 1: Add `notify` crate to Cargo.toml**

```toml
notify = { version = "7", optional = true }

[features]
memory-watcher = ["notify"]
```

- [ ] **Step 2: Implement MemoryWatcher**

Key behavior (spec 9.3):
- Use `notify` crate to watch `~/.gasket/memory/`
- 2-second debounce timer
- Ignore `.history/`, `.tmp`, `_INDEX.md` files
- On settled event: re-embed, update SQLite, regenerate index

```rust
pub struct MemoryWatcher {
    base_dir: PathBuf,
    debounce_ms: u64, // default: 2000
}

pub enum WatchEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
}
```

- [ ] **Step 3: Write tests**

Test with temp directories:
- Create a file → verify event fires after debounce
- Modify a file → verify event fires
- Create `.tmp` file → verify NO event
- Modify `_INDEX.md` → verify NO event

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-storage --features memory-watcher memory::watcher --no-fail-fast`

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/
git commit -m "feat(storage): implement file watcher with debouncing"
```

---

## Phase 8: Agent Loop Integration

### Task 12: Wire MemoryManager into Agent Loop

**Files:**
- Modify: `gasket/engine/src/agent/mod.rs`
- Modify: `gasket/engine/src/agent/loop_.rs`
- Modify: `gasket/engine/src/agent/context.rs`

- [ ] **Step 1: Add MemoryManager to PersistentContext**

In `context.rs`, add optional MemoryManager to PersistentContext:
```rust
pub struct PersistentContext {
    // existing fields...
    pub memory_manager: Option<MemoryManager>,
}
```

- [ ] **Step 2: Inject memory loading into agent pipeline**

In `loop_.rs`, after `inject_system_prompts` and before `assemble_prompt`:
- Call `memory_manager.load_for_context(query)` with current user message
- Append loaded memories to the prompt as a system section
- Log access for each loaded memory

- [ ] **Step 3: Add memory write trigger**

When the agent detects "remember this" or similar signals:
- Call `memory_manager.store.create()` with extracted content
- Auto-determine scenario from context

- [ ] **Step 4: Write integration test**

Test that memory is loaded into agent context correctly within token budget.

- [ ] **Step 5: Run tests**

Run: `cargo test --package gasket-engine agent::memory_manager --no-fail-fast`

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/agent/
git commit -m "feat(engine): wire MemoryManager into agent loop pipeline"
```

---

## Phase 9: Dedup Cron

### Task 13: Cross-Session Deduplication Task

**Files:**
- Create: `gasket/storage/src/memory/dedup.rs`

- [ ] **Step 1: Implement dedup scan**

From spec 9.8:
```rust
pub async fn run_dedup_scan(
    store: &FileMemoryStore,
    embedding_store: &EmbeddingStore,
) -> Result<Vec<DedupReport>> {
    // 1. For each scenario, get all embeddings
    // 2. Pairwise cosine similarity
    // 3. Flag pairs > 0.85 similarity
    // 4. Insert into dedup_reports table
}
```

- [ ] **Step 2: Register as cron job**

Use existing `cron_jobs` table to schedule weekly execution.

- [ ] **Step 3: Write tests**

Test with known similar/dissimilar embeddings.

- [ ] **Step 4: Run tests**

Run: `cargo test --package gasket-storage memory::dedup --no-fail-fast`

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/src/memory/dedup.rs
git commit -m "feat(storage): implement cross-session deduplication cron"
```

---

## Summary

| Phase | Tasks | Key Deliverable |
|-------|-------|----------------|
| 1. Core Types | 1–4 | Scenario, Frequency, frontmatter parsing, path helpers |
| 2. CRUD | 5 | FileMemoryStore with version history |
| 3. Index | 6 | IndexManager with atomic writes |
| 4. Embeddings | 7 | SQLite schema + EmbeddingStore |
| 5. Retrieval | 8–9 | RetrievalEngine + three-phase loading |
| 6. Lifecycle | 10 | Frequency decay/promotion + access log |
| 7. Watcher | 11 | File watcher with debouncing |
| 8. Integration | 12 | Agent loop wiring |
| 9. Dedup | 13 | Cross-session dedup cron |

**Total: 13 tasks, ~9 commits**

Each phase produces independently testable functionality. Phases 1–5 are the minimum viable implementation. Phases 6–9 add operational maturity.
