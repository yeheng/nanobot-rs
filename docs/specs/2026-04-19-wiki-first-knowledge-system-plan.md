# Wiki-First Knowledge System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing memory/ module with a Wiki-first knowledge management system using a three-layer architecture (Raw Sources, Compiled Wiki, Schema).

**Architecture:** The Wiki system stores knowledge in SQLite as the single source of truth (path as PK, content in DB). Disk markdown files are optional derived cache. Three focused structs replace the old WikiStore God Object: `PageStore` (CRUD), `PageIndex` (search), `WikiLog` (audit). Tool names (`memorize`, `memory_search`, `memory_refresh`) are preserved for backward compatibility. A one-time `gasket wiki migrate` command handles data transition.

**Tech Stack:** Rust, tokio, SQLite (sqlx), Tantivy, ONNX embeddings (fastembed), serde_yaml (frontmatter export), chrono

**Spec:** `docs/specs/2026-04-19-wiki-first-knowledge-system-design.md`

---

## File Structure

### New Files (engine crate)

```
gasket/engine/src/wiki/
├── mod.rs                 # Re-exports
├── page.rs                # WikiPage, PageType, PageSummary, PageFilter, slugify
├── store.rs               # PageStore: CRUD + disk sync (replaces WikiStore God Object)
├── index.rs               # PageIndex: Tantivy + FTS5 search (Phase 3)
├── log.rs                 # WikiLog: structured operation log
├── ingest/
│   ├── mod.rs             # IngestPipeline, IngestTier
│   ├── parser.rs          # SourceParser trait + Markdown/HTML/Text parsers
│   ├── extractor.rs       # KnowledgeExtractor (LLM prompt + response parse)
│   ├── integrator.rs      # WikiIntegrator (LLM-driven page updates + cost gate)
│   └── dedup.rs           # SemanticDeduplicator (embedding similarity)
├── query/
│   ├── mod.rs             # WikiQueryEngine (three-phase retrieval)
│   ├── tantivy_adapter.rs # Tantivy index adapter for wiki pages
│   └── reranker.rs        # Hybrid reranking (BM25 + embedding)
├── lint/
│   ├── mod.rs             # WikiLinter, LintReport
│   ├── structural.rs      # Structural checks (orphans, missing, weak)
│   └── semantic.rs        # Semantic checks (contradictions, stale)

gasket/engine/src/hooks/
├── wiki_query.rs          # Replaces memory loading in BeforeRequest
└── (evolution.rs refactored in-place)
```

### New Files (storage crate)

```
gasket/storage/src/wiki/
├── mod.rs                 # Re-exports
├── tables.rs              # SQLite table creation (path as PK, content in DB, no lock table)
├── page_store.rs          # Wiki page CRUD (content lives in SQLite)
├── source_store.rs        # Raw source registry
├── relation_store.rs      # Inter-page relations CRUD
└── log_store.rs           # Structured operation log
```

### New Files (CLI)

```
gasket/cli/src/commands/wiki.rs  # wiki ingest/query/lint/list commands
```

### Modified Files

```
gasket/engine/src/lib.rs                    # pub mod wiki; remove memory refs
gasket/engine/src/session/mod.rs            # PageStore + PageIndex replace MemoryManager
gasket/engine/src/session/config.rs         # WikiConfig replaces MemoryConfig
gasket/engine/src/session/history/builder.rs # PageIndex replaces MemoryLoader
gasket/engine/src/hooks/mod.rs              # Add wiki hooks
gasket/engine/src/hooks/evolution.rs        # Refactor to use PageStore
gasket/engine/src/tools/mod.rs              # Update memory tool implementations
gasket/engine/src/tools/builder.rs          # Re-register memory tools with wiki backing
gasket/engine/src/tools/memorize.rs         # Rewrite: backed by PageStore (keep tool name)
gasket/engine/src/tools/memory_search.rs    # Rewrite: backed by PageIndex (keep tool name)
gasket/engine/src/tools/memory_refresh.rs   # Rewrite: backed by PageIndex (keep tool name)
gasket/cli/src/commands/mod.rs              # Add wiki commands
gasket/cli/src/commands/memory.rs           # Add wiki migrate command
gasket/storage/src/lib.rs                   # pub mod wiki; memory module kept for transition
```

### Deleted Files (Phase 1 end)

```
gasket/engine/src/session/memory/           # Entire directory (MemoryManager facade)
```

**Note:** Tool FILES are kept (memorize.rs, memory_search.rs, memory_refresh.rs) — only their internals are rewritten to use PageStore/PageIndex. Tool NAMES remain `memorize`, `memory_search`, `memory_refresh` for backward compatibility.

**IMPORTANT:** `gasket/storage/src/memory/` is KEPT. It provides `EmbeddingStore`, `FileMemoryStore` patterns that the wiki storage layer reuses. Only the engine-level `session/memory/` facade is removed. Tool file names (`memorize.rs`, `memory_search.rs`, `memory_refresh.rs`) are also kept — only their internals are rewritten.

---

## Phase 1: Foundation

### Task 1: Wiki Data Types

**Files:**
- Create: `gasket/engine/src/wiki/mod.rs`
- Create: `gasket/engine/src/wiki/page.rs`

**No `uri.rs`.** Pages are identified by `String` path. No custom URI scheme.

- [ ] **Step 1: Create wiki page module**

```rust
// gasket/engine/src/wiki/page.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Page type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageType {
    Entity,
    Topic,
    Source,
}

impl PageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Topic => "topic",
            Self::Source => "source",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "entity" => Some(Self::Entity),
            "topic" => Some(Self::Topic),
            "source" => Some(Self::Source),
            _ => None,
        }
    }
}

/// A wiki page. One struct. One constructor. No special cases.
/// SQLite is the single truth source. Disk files are derived cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPage {
    /// Relative path under wiki root: "entities/projects/gasket"
    pub path: String,
    pub title: String,
    pub page_type: PageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub source_count: u32,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 { 1.0 }

impl WikiPage {
    /// One constructor. All page types go through this.
    pub fn new(path: String, title: String, page_type: PageType, content: String) -> Self {
        let now = Utc::now();
        Self {
            path,
            title,
            page_type,
            content,
            category: None,
            tags: vec![],
            created: now,
            updated: now,
            source_count: 0,
            confidence: 1.0,
        }
    }

    /// Helper: build a path from parts: ["entities", "projects", "gasket"]
    pub fn make_path(parts: &[&str]) -> String {
        parts.join("/")
    }

    /// Convert to markdown for disk export (optional cache)
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&serde_yaml::to_string(&self).unwrap_or_default());
        out.push_str("---\n\n");
        out.push_str(&self.content);
        out
    }

    /// Parse from markdown (used only for migration / disk cache rebuild)
    pub fn from_markdown(path: String, markdown: &str) -> anyhow::Result<Self> {
        let content = markdown.trim_start();
        if !content.starts_with("---") {
            anyhow::bail!("missing frontmatter delimiter");
        }
        let rest = &content[3..];
        let end = rest.find("\n---").ok_or_else(|| anyhow::anyhow!("unclosed frontmatter"))?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').trim_start();
        let mut page: WikiPage = serde_yaml::from_str(yaml)?;
        page.path = path;
        page.content = body.to_string();
        Ok(page)
    }
}

/// Summary for listing (no content — lightweight)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub path: String,
    pub title: String,
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub updated: DateTime<Utc>,
    pub confidence: f64,
}

/// Filter for listing pages
#[derive(Debug, Clone, Default)]
pub struct PageFilter {
    pub page_type: Option<PageType>,
    pub category: Option<String>,
    pub tags: Vec<String>,
}

/// Slugify a string for use in paths
pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
```

- [ ] **Step 2: Create wiki module root with tests**

```rust
// gasket/engine/src/wiki/mod.rs

pub mod page;

// Re-exports
pub use page::{PageFilter, PageSummary, PageType, WikiPage, slugify};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_new_entity() {
        let page = WikiPage::new(
            "entities/projects/gasket".to_string(),
            "Gasket Project".to_string(),
            PageType::Entity,
            "A Rust agent framework.".to_string(),
        );
        assert_eq!(page.path, "entities/projects/gasket");
        assert_eq!(page.page_type, PageType::Entity);
        assert_eq!(page.confidence, 1.0);
        assert!(page.tags.is_empty());
    }

    #[test]
    fn test_page_new_topic() {
        let page = WikiPage::new(
            "topics/rust-async".to_string(),
            "Rust Async".to_string(),
            PageType::Topic,
            "How async works in Rust.".to_string(),
        );
        assert_eq!(page.page_type, PageType::Topic);
    }

    #[test]
    fn test_page_markdown_roundtrip() {
        let mut page = WikiPage::new(
            "topics/test".to_string(),
            "Test Topic".to_string(),
            PageType::Topic,
            "Some content here.".to_string(),
        );
        page.tags = vec!["test".to_string()];
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown("topics/test".to_string(), &md).unwrap();
        assert_eq!(parsed.title, "Test Topic");
        assert_eq!(parsed.content, "Some content here.");
        assert_eq!(parsed.tags, vec!["test"]);
    }

    #[test]
    fn test_make_path() {
        assert_eq!(WikiPage::make_path(&["entities", "projects", "gasket"]), "entities/projects/gasket");
        assert_eq!(WikiPage::make_path(&["topics", "rust-async"]), "topics/rust-async");
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Rust & LLM"), "rust-llm");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn test_page_type_roundtrip() {
        assert_eq!(PageType::from_str("entity"), Some(PageType::Entity));
        assert_eq!(PageType::from_str("topic"), Some(PageType::Topic));
        assert_eq!(PageType::from_str("source"), Some(PageType::Source));
        assert_eq!(PageType::from_str("unknown"), None);
    }
}
```

- [ ] **Step 3: Run tests to verify**

Run: `cargo test --package gasket-engine -- wiki::tests --nocapture`
Expected: All 6 tests PASS

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/wiki/
git commit -m "feat(wiki): add WikiPage data type with path-based identity"
```

---

### Task 2: SQLite Wiki Tables

**Files:**
- Create: `gasket/storage/src/wiki/mod.rs`
- Create: `gasket/storage/src/wiki/tables.rs`
- Create: `gasket/storage/src/wiki/page_store.rs`
- Create: `gasket/storage/src/wiki/source_store.rs`
- Create: `gasket/storage/src/wiki/relation_store.rs`
- Create: `gasket/storage/src/wiki/log_store.rs`
- Modify: `gasket/storage/src/lib.rs` (add `pub mod wiki;`)

- [ ] **Step 1: Create wiki storage module**

```rust
// gasket/storage/src/wiki/mod.rs

pub mod log_store;
pub mod page_store;
pub mod relation_store;
pub mod source_store;
pub mod tables;

pub use log_store::WikiLogStore;
pub use page_store::WikiPageStore;
pub use relation_store::WikiRelationStore;
pub use source_store::WikiSourceStore;
```

- [ ] **Step 2: Create table definitions**

```rust
// gasket/storage/src/wiki/tables.rs

use sqlx::SqlitePool;

/// Create all wiki-related tables.
/// Key design: wiki_pages.path is PK, content lives in SQLite (single truth).
/// No wiki_page_locks table — SQLite WAL handles concurrency.
pub async fn create_wiki_tables(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_pages (
            path        TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            type        TEXT NOT NULL,
            category    TEXT,
            tags        TEXT,
            content     TEXT NOT NULL DEFAULT '',
            created     TEXT NOT NULL,
            updated     TEXT NOT NULL,
            source_count INTEGER DEFAULT 0,
            confidence  REAL DEFAULT 1.0,
            checksum    TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS raw_sources (
            id          TEXT PRIMARY KEY,
            path        TEXT NOT NULL,
            format      TEXT NOT NULL,
            ingested    INTEGER DEFAULT 0,
            ingested_at TEXT,
            title       TEXT,
            metadata    TEXT,
            created     TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_relations (
            from_page   TEXT NOT NULL,
            to_page     TEXT NOT NULL,
            relation    TEXT NOT NULL,
            confidence  REAL DEFAULT 1.0,
            created     TEXT NOT NULL,
            PRIMARY KEY (from_page, to_page, relation)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            action      TEXT NOT NULL,
            target      TEXT,
            detail      TEXT,
            created     TEXT NOT NULL DEFAULT (datetime('now'))
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_type ON wiki_pages(type)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_category ON wiki_pages(category)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_updated ON wiki_pages(updated)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_raw_sources_ingested ON raw_sources(ingested)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_log_action ON wiki_log(action)")
        .execute(pool).await?;

    Ok(())
}
```

- [ ] **Step 3: Create page store**

```rust
// gasket/storage/src/wiki/page_store.rs

use anyhow::Result;
use sqlx::SqlitePool;

/// SQLite-backed wiki page store. Single source of truth.
/// Content lives here. Disk files are optional cache.
pub struct WikiPageStore {
    pool: SqlitePool,
}

impl WikiPageStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Atomic UPSERT. SQLite WAL handles concurrency. No separate lock needed.
    pub async fn upsert(
        &self,
        path: &str,
        title: &str,
        page_type: &str,
        category: Option<&str>,
        tags: &str,
        content: &str,
        source_count: u32,
        confidence: f64,
        checksum: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO wiki_pages (path, title, type, category, tags, content, created, updated, source_count, confidence, checksum)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                title = excluded.title,
                type = excluded.type,
                category = excluded.category,
                tags = excluded.tags,
                content = excluded.content,
                updated = excluded.updated,
                source_count = excluded.source_count,
                confidence = excluded.confidence,
                checksum = excluded.checksum
            "#,
        )
        .bind(path).bind(title).bind(page_type)
        .bind(category).bind(tags).bind(content)
        .bind(&now).bind(&now)
        .bind(source_count).bind(confidence).bind(checksum)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, path: &str) -> Result<Option<PageRow>> {
        let row = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum FROM wiki_pages WHERE path = ?"
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_pages WHERE path = ?")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT path FROM wiki_pages WHERE path = ?"
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    pub async fn list_by_type(&self, page_type: &str) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum FROM wiki_pages WHERE type = ? ORDER BY updated DESC"
        )
        .bind(page_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_all(&self) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum FROM wiki_pages ORDER BY updated DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct PageRow {
    pub path: String,
    pub title: String,
    #[sqlx(rename = "type")]
    pub page_type: String,
    pub category: Option<String>,
    pub tags: Option<String>,
    pub content: String,
    pub created: String,
    pub updated: String,
    pub source_count: i64,
    pub confidence: f64,
    pub checksum: Option<String>,
}
```

- [ ] **Step 4: Create source, relation, and log stores**

These follow the same pattern. Key methods:
- `WikiSourceStore`: `register()`, `mark_ingested()`, `list_uningested()`
- `WikiRelationStore`: `add()`, `get_outgoing()`, `get_incoming()`, `delete_all_for_page()`
- `WikiLogStore`: `append()`, `list_recent()`

(Full implementations follow the same `sqlx::query` pattern as page_store.)

- [ ] **Step 5: Add `pub mod wiki;` to storage lib.rs**

- [ ] **Step 6: Run tests**

Run: `cargo test --package gasket-storage`
Expected: Existing tests still pass (new tables are additive)

- [ ] **Step 7: Commit**

```bash
git add gasket/storage/src/wiki/ gasket/storage/src/lib.rs
git commit -m "feat(storage): add wiki SQLite tables and page/source/relation/log stores"
```

---

### Task 3: PageStore, PageIndex, WikiLog (split from WikiStore)

**Files:**
- Create: `gasket/engine/src/wiki/store.rs`
- Create: `gasket/engine/src/wiki/index.rs`
- Create: `gasket/engine/src/wiki/log.rs`
- Modify: `gasket/engine/src/wiki/mod.rs` (add modules)

**No monolithic WikiStore.** Three focused structs, each with one `SqlitePool` reference.

- [ ] **Step 1: Write PageStore (CRUD + disk sync)**

```rust
// gasket/engine/src/wiki/store.rs

use anyhow::Result;
use gasket_storage::wiki::WikiPageStore;
use std::path::PathBuf;
use tokio::fs;

use super::page::{PageFilter, PageSummary, PageType, WikiPage};

/// PageStore: CRUD operations on wiki pages.
/// SQLite is single truth. Disk files are optional cache.
pub struct PageStore {
    db: WikiPageStore,
    wiki_root: PathBuf,
}

impl PageStore {
    pub fn new(pool: sqlx::SqlitePool, wiki_root: PathBuf) -> Self {
        Self {
            db: WikiPageStore::new(pool),
            wiki_root,
        }
    }

    /// Ensure wiki directory structure exists
    pub async fn init_dirs(&self) -> Result<()> {
        for dir in &["entities/people", "entities/projects", "entities/concepts", "topics", "sources"] {
            fs::create_dir_all(self.wiki_root.join(dir)).await?;
        }
        Ok(())
    }

    /// Read a page from SQLite (single truth)
    pub async fn read(&self, path: &str) -> Result<WikiPage> {
        let row = self.db.get(path).await?
            .ok_or_else(|| anyhow::anyhow!("page not found: {}", path))?;
        Ok(WikiPage {
            path: row.path,
            title: row.title,
            page_type: PageType::from_str(&row.page_type)
                .unwrap_or(PageType::Topic),
            category: row.category,
            tags: row.tags.as_ref()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default(),
            content: row.content,
            created: chrono::DateTime::parse_from_rfc3339(&row.created)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            updated: chrono::DateTime::parse_from_rfc3339(&row.updated)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            source_count: row.source_count as u32,
            confidence: row.confidence,
        })
    }

    /// Write a page to SQLite (single truth) + optional disk sync
    pub async fn write(&self, page: &WikiPage) -> Result<()> {
        let tags_str = serde_json::to_string(&page.tags)?;
        let checksum = Some(&format!("{:x}", md5::compute(&page.content))[..8] as &str);
        self.db.upsert(
            &page.path,
            &page.title,
            page.page_type.as_str(),
            page.category.as_deref(),
            &tags_str,
            &page.content,
            page.source_count,
            page.confidence,
            None, // checksum computed from content
        ).await?;

        // Lazy disk sync (best effort)
        let _ = self.sync_to_disk(page).await;
        Ok(())
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.db.delete(path).await?;
        // Best-effort disk cleanup
        let disk_path = self.wiki_root.join(format!("{}.md", path));
        let _ = fs::remove_file(&disk_path).await;
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.db.exists(path).await
    }

    pub async fn list(&self, filter: PageFilter) -> Result<Vec<PageSummary>> {
        let rows = match &filter.page_type {
            Some(pt) => self.db.list_by_type(pt.as_str()).await?,
            None => self.db.list_all().await?,
        };
        Ok(rows.iter().map(|r| PageSummary {
            path: r.path.clone(),
            title: r.title.clone(),
            page_type: PageType::from_str(&r.page_type).unwrap_or(PageType::Topic),
            category: r.category.clone(),
            tags: r.tags.as_ref()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default(),
            updated: chrono::DateTime::parse_from_rfc3339(&r.updated)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            confidence: r.confidence,
        }).collect())
    }

    /// Sync page to disk as markdown (optional, for human readability)
    pub async fn sync_to_disk(&self, page: &WikiPage) -> Result<()> {
        let path = self.wiki_root.join(format!("{}.md", page.path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, page.to_markdown()).await?;
        Ok(())
    }

    /// Rebuild disk cache from SQLite (for migration or recovery)
    pub async fn rebuild_disk_cache(&self) -> Result<usize> {
        let rows = self.db.list_all().await?;
        for row in &rows {
            let page = self.read(&row.path).await?;
            self.sync_to_disk(&page).await?;
        }
        Ok(rows.len())
    }

    // --- Relations ---

    pub async fn add_relation(&self, from: &str, to: &str, relation: &str) -> Result<()> {
        // Delegated to WikiRelationStore via storage layer
        // self.relation_store.add(from, to, relation).await
        todo!("Implemented in storage/src/wiki/relation_store.rs")
    }

    pub async fn get_outgoing(&self, path: &str) -> Result<Vec<(String, String, String)>> {
        todo!("Implemented in storage/src/wiki/relation_store.rs")
    }
}
```

- [ ] **Step 2: Write PageIndex (search stub)**

```rust
// gasket/engine/src/wiki/index.rs

use anyhow::Result;

use super::page::PageSummary;

/// PageIndex: search over wiki pages.
/// Phase 1: SQLite FTS5 only. Phase 3: Tantivy + embedding.
pub struct PageIndex {
    // pool: SqlitePool,  -- added in Phase 3
}

impl PageIndex {
    pub fn new() -> Self {
        Self {}
    }

    /// Phase 1: placeholder. Returns empty results.
    /// Phase 3: Tantivy BM25 + embedding rerank.
    pub async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<PageSummary>> {
        Ok(vec![])
    }
}
```

- [ ] **Step 3: Write WikiLog (structured log)**

```rust
// gasket/engine/src/wiki/log.rs

use anyhow::Result;
use gasket_storage::wiki::WikiLogStore;
use serde::{Deserialize, Serialize};

/// WikiLog: structured operation log.
/// Data in SQLite. No log.md file maintenance.
pub struct WikiLog {
    db: WikiLogStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub created: String,
}

impl WikiLog {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { db: WikiLogStore::new(pool) }
    }

    pub async fn append(&self, action: &str, target: &str, detail: &str) -> Result<()> {
        self.db.append(action, target, detail).await
    }

    pub async fn list_recent(&self, limit: usize) -> Result<Vec<LogEntry>> {
        self.db.list_recent(limit).await
    }
}
```

- [ ] **Step 4: Update wiki module root**

```rust
// gasket/engine/src/wiki/mod.rs (updated)

pub mod page;
pub mod store;
pub mod index;
pub mod log;

// Re-exports
pub use page::{PageFilter, PageSummary, PageType, WikiPage, slugify};
pub use store::PageStore;
pub use index::PageIndex;
pub use log::WikiLog;

// Tests remain in this file (unchanged from Task 1)
#[cfg(test)]
mod tests { /* ... same as Task 1 ... */ }
```

- [ ] **Step 5: Add wiki module to engine lib.rs**

Add `pub mod wiki;` to `gasket/engine/src/lib.rs`.

- [ ] **Step 6: Run build**

Run: `cargo build --package gasket-engine`
Expected: Compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/wiki/ gasket/engine/src/lib.rs
git commit -m "feat(wiki): add PageStore, PageIndex, WikiLog (split from WikiStore)"
```

---

### Task 4: WikiConfig and Session Integration

**Files:**
- Modify: `gasket/engine/src/session/config.rs` (add WikiConfig)
- Modify: `gasket/engine/src/session/mod.rs` (PageStore + PageIndex replace MemoryManager)

- [ ] **Step 1: Add WikiConfig to session config**

Add to `gasket/engine/src/session/config.rs`:

```rust
use std::path::PathBuf;

// Default helper functions
fn default_true() -> bool { true }
fn default_batch_size() -> usize { 20 }
fn default_dedup_threshold() -> f64 { 0.85 }
fn default_max_pages() -> usize { 15 }
fn default_limit() -> usize { 10 }
fn default_lint_interval() -> String { "24h".to_string() }
fn default_max_cost() -> f64 { 0.10 }
fn default_cost_warning() -> f64 { 0.05 }

fn default_wiki_base() -> String {
    dirs::home_dir()
        .map(|p| p.join(".gasket/wiki").to_str().unwrap().to_string())
        .unwrap_or_else(|| "~/.gasket/wiki".to_string())
}

fn default_sources_base() -> String {
    dirs::home_dir()
        .map(|p| p.join(".gasket/sources").to_str().unwrap().to_string())
        .unwrap_or_else(|| "~/.gasket/sources".to_string())
}

/// Wiki system configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WikiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_wiki_base")]
    pub base_path: String,
    #[serde(default = "default_sources_base")]
    pub sources_path: String,
    #[serde(default)]
    pub ingest: WikiIngestConfig,
    #[serde(default)]
    pub query: WikiQueryConfig,
    #[serde(default)]
    pub lint: WikiLintConfig,
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_path: default_wiki_base(),
            sources_path: default_sources_base(),
            ingest: WikiIngestConfig::default(),
            query: WikiQueryConfig::default(),
            lint: WikiLintConfig::default(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WikiIngestConfig {
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_true")]
    pub auto_ingest: bool,
    #[serde(default = "default_dedup_threshold")]
    pub dedup_threshold: f64,
    #[serde(default = "default_max_pages")]
    pub max_pages_per_ingest: usize,
    #[serde(default = "default_max_cost")]
    pub max_cost_per_ingest: f64,
    #[serde(default = "default_cost_warning")]
    pub cost_warning_threshold: f64,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            batch_size: 20, auto_ingest: true, dedup_threshold: 0.85,
            max_pages_per_ingest: 15, max_cost_per_ingest: 0.10,
            cost_warning_threshold: 0.05,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WikiQueryConfig {
    #[serde(default = "default_limit")]
    pub default_limit: usize,
    #[serde(default = "default_true")]
    pub hybrid_search: bool,
    #[serde(default = "default_true")]
    pub answer_filing: bool,
}

impl Default for WikiQueryConfig {
    fn default() -> Self { Self { default_limit: 10, hybrid_search: true, answer_filing: true } }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WikiLintConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_lint_interval")]
    pub interval: String,
    #[serde(default = "default_true")]
    pub auto_fix: bool,
    #[serde(default = "default_true")]
    pub semantic_checks: bool,
}

impl Default for WikiLintConfig {
    fn default() -> Self {
        Self { enabled: true, interval: "24h".to_string(), auto_fix: true, semantic_checks: true }
    }
}
```

Also add to `AgentConfig`:
```rust
pub wiki: Option<WikiConfig>,  // Replaces embedding_config + memory_budget + evolution
```

- [ ] **Step 2: Replace MemoryManager with PageStore + PageIndex in session**

In `gasket/engine/src/session/mod.rs`:
- Change `memory_manager: Option<Arc<MemoryManager>>` to `page_store: Option<Arc<PageStore>>`
- Add `page_index: Option<Arc<PageIndex>>`
- Add `wiki_log: Option<Arc<WikiLog>>`

Replace `try_init_memory_manager()` with `try_init_wiki()`:

```rust
fn try_init_wiki(config: &AgentConfig, pool: SqlitePool) -> Option<(Arc<PageStore>, Arc<PageIndex>, Arc<WikiLog>)> {
    let wiki_config = config.wiki.as_ref()?;
    if !wiki_config.enabled {
        return None;
    }
    let wiki_base = PathBuf::from(&wiki_config.base_path);
    let store = PageStore::new(pool.clone(), wiki_base);
    let index = PageIndex::new();  // Phase 1: no Tantivy
    let log = WikiLog::new(pool.clone());

    // Ensure directory structure + SQLite tables exist
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            store.init_dirs().await.ok()?;
            gasket_storage::wiki::tables::create_wiki_tables(&pool).await.ok()
        })
    })?;

    Some((Arc::new(store), Arc::new(index), Arc::new(log)))
}
```

Update accessors: `pub fn page_store()`, `pub fn page_index()`, `pub fn wiki_log()`.

- [ ] **Step 3: Build and fix compile errors**

Run: `cargo build --package gasket-engine 2>&1 | head -50`
Expected: Compile errors in tools/ and hooks/ (fixed in Tasks 5-6)

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/session/
git commit -m "refactor(session): replace MemoryManager with PageStore + PageIndex + WikiLog"
```

---

### Task 5: Rewrite Memory Tools (keep tool names)

**Files:**
- Modify: `gasket/engine/src/tools/memorize.rs` (rewrite internals, keep name `memorize`)
- Modify: `gasket/engine/src/tools/memory_search.rs` (rewrite internals, keep name `memory_search`)
- Modify: `gasket/engine/src/tools/memory_refresh.rs` (rewrite internals, keep name `memory_refresh`)
- Modify: `gasket/engine/src/tools/builder.rs` (update registration)
- Modify: `gasket/engine/src/tools/mod.rs` (update exports)

**CRITICAL: Tool names stay as `memorize`, `memory_search`, `memory_refresh`.** The LLM's existing tool-use patterns must not break. Only the backing implementation changes from MemoryManager to PageStore/PageIndex.

- [ ] **Step 1: Rewrite memorize.rs internals**

Replace `MemoryManager.create_memory()` calls with `PageStore::write()`. Tool name remains `memorize`.

```rust
// Key change in execute():
let path = format!("entities/{}/{}", slugify(&scenario), slugify(&title));
let mut page = WikiPage::new(path, title, page_type, content);
page.tags = tags;

ctx.page_store().write(&page).await?;
```

- [ ] **Step 2: Rewrite memory_search.rs internals**

Replace `MemoryManager.search()` calls with `PageStore::list()` (Phase 1) / `PageIndex::search()` (Phase 3). Tool name remains `memory_search`.

- [ ] **Step 3: Rewrite memory_refresh.rs internals**

Replace `MemoryManager.refresh()` calls with `PageIndex::rebuild()` (Phase 3 stub). Tool name remains `memory_refresh`.

- [ ] **Step 4: Update tool builder registration**

In `gasket/engine/src/tools/builder.rs`:
- Update tool constructors to receive `Arc<PageStore>` + `Arc<PageIndex>` instead of `Arc<MemoryManager>`
- Tool names and descriptions remain unchanged

- [ ] **Step 5: Update tools/mod.rs exports**

- [ ] **Step 6: Build and verify**

Run: `cargo build --package gasket-engine`
Expected: Compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/tools/
git commit -m "refactor(tools): rewrite memory tools to use PageStore (keep tool names)"
```

---

### Task 6: Update Hooks

**Files:**
- Modify: `gasket/engine/src/hooks/evolution.rs` (use PageStore instead of MemoryManager)
- Create: `gasket/engine/src/hooks/wiki_query.rs` (replace MemoryLoader hook)
- Modify: `gasket/engine/src/hooks/mod.rs` (add new hook)

- [ ] **Step 1: Refactor EvolutionHook to use PageStore**

Change `evolution.rs` to:
- Accept `Arc<PageStore>` instead of `Arc<MemoryManager>`
- Convert extracted knowledge items to WikiPages

Concrete conversion code (replace the `create_memory` calls):

```rust
// BEFORE (current evolution.rs):
//   memory_manager.create_memory(scenario, &mem.title, &tags, Frequency::Warm, &mem.content).await
//
// AFTER (wiki):
for item in &items {
    let path = format!("entities/{}/{}", slugify(&item.scenario), slugify(&item.title));
    if store.exists(&path).await? {
        continue; // Quick dedup
    }
    let mut page = WikiPage::new(
        path,
        item.title.clone(),
        match item.scenario {
            Scenario::Profile => PageType::Entity,
            Scenario::Knowledge => PageType::Topic,
            Scenario::Decisions => PageType::Entity,
            _ => PageType::Topic,
        },
        item.content.clone(),
    );
    page.tags = item.tags.clone();
    page.tags.push("auto_learned".to_string());
    store.write(&page).await?;
}
```

Note: The LLM extraction prompt stays unchanged. Only the storage layer changes.

- [ ] **Step 2: Create WikiQueryHook**

```rust
// gasket/engine/src/hooks/wiki_query.rs
// BeforeRequest hook that loads relevant wiki pages into context
// Uses PageIndex::search() for retrieval
```

- [ ] **Step 3: Register hooks and build**

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/hooks/
git commit -m "refactor(hooks): update EvolutionHook for PageStore, add WikiQueryHook"
```

---

### Task 7: Remove Memory Module

**Files:**
- Delete: `gasket/engine/src/session/memory/` (entire directory)
- Modify: `gasket/engine/src/session/mod.rs` (remove `pub mod memory;`)
- Modify: `gasket/engine/src/lib.rs` (remove memory re-exports)

**Tool files are KEPT** (`memorize.rs`, `memory_search.rs`, `memory_refresh.rs`) — only their internals were rewritten in Task 5. The storage-level `gasket/storage/src/memory/` is also KEPT.

- [ ] **Step 1: Comprehensive search for ALL memory references**

```bash
grep -rn "MemoryManager\|MemoryLoader\|MemoryWriter\|MemoryContext\|MemoryStore\|MemoryConfig\|memory_manager\|memory::\|mod memory" gasket/engine/src/ --include="*.rs"
grep -rn "use gasket_storage::memory" gasket/engine/src/ --include="*.rs"
```

Verify: References should only exist in `session/memory/` (to be deleted) and storage-level code (to be kept).

- [ ] **Step 2: Delete engine memory directory only**

```bash
rm -rf gasket/engine/src/session/memory/
```

**DO NOT delete** `memorize.rs`, `memory_search.rs`, `memory_refresh.rs` — they were rewritten in Task 5.

- [ ] **Step 3: Remove all memory declarations and re-exports**

In `gasket/engine/src/session/mod.rs`:
- Remove `pub mod memory;`
- Remove `pub use memory::{MemoryContext, MemoryManager};`
- Remove `pub use store::{MemoryProvider, MemoryStore};`

In `gasket/engine/src/lib.rs`:
- Remove `MemoryManager, MemoryContext` from session re-exports
- Remove any `pub mod memory` facade block that re-exports storage/memory

In `gasket/engine/src/session/history/builder.rs`:
- Remove `MemoryLoader` field and usage (replaced by PageIndex in Phase 3)

- [ ] **Step 4: Fix remaining compile errors**

Search for any remaining references and replace with wiki equivalents. Key files to check:
- `gasket/engine/src/session/store.rs` (MemoryStore type)
- `gasket/engine/src/tools/builder.rs` (already updated in Task 5)
- `gasket/cli/src/commands/memory.rs` (CLI imports)

- [ ] **Step 5: Build**

Run: `cargo build --package gasket-engine`
Expected: Clean build, no errors

- [ ] **Step 6: Run tests**

Run: `cargo test --package gasket-engine`
Expected: All tests pass (memory-specific tests are gone, wiki tests pass)

- [ ] **Step 7: Commit**

```bash
git add -A gasket/engine/src/
git commit -m "refactor(engine): remove session/memory/ module, fully replaced by wiki/"
```

---

### Task 8: CLI Commands + Migration

**Files:**
- Modify: `gasket/cli/src/commands/memory.rs` (rewrite as wiki commands)
- Modify: `gasket/cli/src/commands/mod.rs` (add wiki commands)
- Modify: `gasket/cli/src/main.rs` (update CLI dispatch)

- [ ] **Step 1: Rewrite CLI commands**

Replace `memory refresh` / `memory decay` with:
- `wiki ingest <path>` - Import a document
- `wiki search <query>` - Search wiki pages
- `wiki list` - List all wiki pages
- `wiki lint` - Run health check
- `wiki migrate` - One-time migration from old memory system

- [ ] **Step 2: Implement `wiki migrate` command**

```rust
// In gasket/cli/src/commands/wiki.rs

/// One-time migration: reads old memory_metadata table → creates wiki pages
async fn migrate(pool: &SqlitePool) -> Result<()> {
    // 1. Read old memory_metadata rows
    let old_rows = sqlx::query_as::<_, OldMemoryRow>(
        "SELECT scenario, title, content, tags, frequency FROM memory_metadata"
    )
    .fetch_all(pool)
    .await?;

    if old_rows.is_empty() {
        println!("No old memory data found. Nothing to migrate.");
        return Ok(());
    }

    // 2. Convert to WikiPages and write to wiki_pages table
    let store = PageStore::new(pool.clone(), wiki_root);
    let mut migrated = 0;
    for row in &old_rows {
        let path = format!("entities/{}/{}", slugify(&row.scenario), slugify(&row.title));
        if store.exists(&path).await? {
            continue; // Skip duplicates
        }
        let mut page = WikiPage::new(
            path,
            row.title.clone(),
            match row.scenario.as_str() {
                "profile" => PageType::Entity,
                "knowledge" => PageType::Topic,
                "decisions" => PageType::Entity,
                _ => PageType::Topic,
            },
            row.content.clone(),
        );
        page.tags = row.tags.clone();
        page.tags.push("migrated".to_string());
        store.write(&page).await?;
        migrated += 1;
    }

    println!("Migrated {} memories to wiki pages.", migrated);

    // 3. Log migration
    let log = WikiLog::new(pool.clone());
    log.append("migrate", &format!("{}_pages", migrated), "memory_metadata -> wiki_pages").await?;

    Ok(())
}
```

- [ ] **Step 3: Update CLI dispatch in main.rs**

- [ ] **Step 4: Build CLI**

Run: `cargo build --package gasket-cli`
Expected: Clean build (ignoring pre-existing tui.rs errors)

- [ ] **Step 5: Commit**

```bash
git add gasket/cli/src/
git commit -m "refactor(cli): add wiki commands with migration from old memory system"
```

---

## Phase 2: Ingest Pipeline

### Task 9: Source Parsers

**Files:**
- Create: `gasket/engine/src/wiki/ingest/mod.rs`
- Create: `gasket/engine/src/wiki/ingest/parser.rs`

- [ ] **Step 1: Define SourceParser trait and implementations**

Implement `SourceParser` for: Markdown, PlainText, Html, Conversation (session events).

Markdown parser reads the file directly. HTML parser strips tags. Conversation parser serializes SessionEvents.

- [ ] **Step 2: Test parsers**

- [ ] **Step 3: Commit**

---

### Task 10: Knowledge Extractor (LLM)

**Files:**
- Create: `gasket/engine/src/wiki/ingest/extractor.rs`

- [ ] **Step 1: Implement LLM-based knowledge extraction**

Prompt LLM to extract: key entities, concepts, claims, and relationships from parsed source content. Returns structured `ExtractedKnowledge`.

- [ ] **Step 2: Test with mock LLM**

- [ ] **Step 3: Commit**

---

### Task 11: Wiki Integrator (LLM-driven + Cost Gate)

**Files:**
- Create: `gasket/engine/src/wiki/ingest/integrator.rs`

- [ ] **Step 1: Implement WikiIntegrator with two-tier API + cost validation**

```rust
pub enum IngestTier {
    Quick,  // 1 page, low LLM cost (conversation/agent exploration)
    Deep,   // 10-15 pages, full integration (document import)
}

/// Cost estimate for a deep ingest operation
pub struct CostEstimate {
    pub estimated_input_tokens: u32,
    pub estimated_pages_affected: usize,
    pub estimated_cost_usd: f64,
}

pub struct IngestConfig {
    pub max_cost_per_ingest: f64,      // USD, default 0.10
    pub cost_warning_threshold: f64,   // USD, default 0.05
}

impl WikiIntegrator {
    /// Quick ingest: creates 1 entity/topic page. No LLM needed.
    pub async fn quick_ingest(&self, title: &str, content: &str, tags: Vec<String>, store: &PageStore) -> Result<IngestReport> {
        let path = format!("topics/{}", slugify(title));
        let mut page = WikiPage::new(path, title.to_string(), PageType::Topic, content.to_string());
        page.tags = tags;
        store.write(&page).await?;
        Ok(IngestReport::quick(page))
    }

    /// Estimate token cost for a deep ingest before executing it
    pub fn estimate_cost(&self, source: &ParsedSource, store: &PageStore) -> CostEstimate {
        let source_tokens = estimate_tokens(&source.content);
        // Rough estimate: each affected page costs ~2k input tokens + ~500 relation tokens
        let estimated_pages = std::cmp::min(source.entities.len() * 2, 15);
        let update_tokens = estimated_pages as u32 * 2500;

        CostEstimate {
            estimated_input_tokens: source_tokens + update_tokens,
            estimated_pages_affected: estimated_pages,
            estimated_cost_usd: self.pricing.calculate(&self.model, source_tokens + update_tokens),
        }
    }

    /// Deep ingest with cost validation gate.
    /// LLM analyzes impact on 10-15 existing pages, updates them.
    pub async fn deep_ingest(&self, source: &ParsedSource, store: &PageStore) -> Result<IngestReport> {
        // GATE: validate cost before proceeding
        let estimate = self.estimate_cost(source, store);
        if estimate.estimated_cost_usd > self.config.max_cost_per_ingest {
            anyhow::bail!(
                "Deep ingest estimated cost ${:.4} exceeds budget ${:.4}. \
                 Affected pages: {}. Use quick_ingest or increase budget.",
                estimate.estimated_cost_usd, self.config.max_cost_per_ingest,
                estimate.estimated_pages_affected
            );
        }
        if estimate.estimated_cost_usd > self.config.cost_warning_threshold {
            tracing::warn!(
                "Deep ingest cost ${:.4} above warning threshold ${:.4}. Proceeding.",
                estimate.estimated_cost_usd, self.config.cost_warning_threshold
            );
        }

        // Proceed with deep ingest
        let affected_pages = self.llm_analyze_impact(source, store).await?;
        let source_path = format!("sources/{}", slugify(&source.title));
        let source_page = WikiPage::new(
            source_path,
            source.title.clone(),
            PageType::Source,
            source.content.clone(),
        );
        store.write(&source_page).await?;

        for page_path in &affected_pages {
            let page = store.read(page_path).await?;
            let updated = self.llm_update_page(&page, source).await?;
            store.write(&updated).await?;
        }

        Ok(IngestReport::deep(source_page, affected_pages))
    }
}
```

- [ ] **Step 2: Test integrator**

- [ ] **Step 3: Commit**

---

### Task 12: Semantic Deduplicator

**Files:**
- Create: `gasket/engine/src/wiki/ingest/dedup.rs`

- [ ] **Step 1: Implement embedding-based deduplication**

Compare new knowledge against existing wiki page embeddings. Skip if similarity > threshold (0.85).

- [ ] **Step 2: Test dedup**

- [ ] **Step 3: Commit**

---

### Task 13: Index and Log Maintenance

**Files:**
- Create: `gasket/engine/src/wiki/index.rs`
- Create: `gasket/engine/src/wiki/log.rs`

- [ ] **Step 1: Implement index.md generation**

Scan all pages, generate categorized markdown index with links and summaries.

- [ ] **Step 2: Implement log.md append**

Structured, parseable log entries appended on every operation.

- [ ] **Step 3: Test**

- [ ] **Step 4: Commit**

---

## Phase 3: Query Pipeline

### Task 14: Tantivy Adapter

**Files:**
- Create: `gasket/engine/src/wiki/query/mod.rs`
- Create: `gasket/engine/src/wiki/query/tantivy_adapter.rs`

- [ ] **Step 1: Define wiki Tantivy schema**

**Note:** Use the `tantivy` crate directly (already in workspace dependencies), NOT `gasket-tantivy` (which is a CLI tool, not a library). Create the adapter from scratch.

Per spec Section 10: id, path, title, content, summary, type, category, tags, created, updated, confidence, source_count.

Index location: `~/.gasket/wiki/.tantivy/`

- [ ] **Step 2: Implement document upsert on page write**

- [ ] **Step 3: Implement BM25 search**

- [ ] **Step 4: Test with sample pages**

- [ ] **Step 5: Commit**

---

### Task 15: WikiQueryEngine

**Files:**
- Create: `gasket/engine/src/wiki/query/reranker.rs`
- Modify: `gasket/engine/src/wiki/query/mod.rs`

- [ ] **Step 1: Implement three-phase query**

Tantivy candidates → embedding rerank → budget selection → load pages.

- [ ] **Step 2: Implement answer filing**

New topic page from good Q&A pairs.

- [ ] **Step 3: Integrate with WikiQueryHook**

- [ ] **Step 4: Test end-to-end**

- [ ] **Step 5: Commit**

---

## Phase 4: Lint Pipeline

### Task 16: Structural Lint

**Files:**
- Create: `gasket/engine/src/wiki/lint/mod.rs`
- Create: `gasket/engine/src/wiki/lint/structural.rs`

- [ ] **Step 1: Implement orphan page detection**

Pages with no incoming links (not in any outgoing_links).

- [ ] **Step 2: Implement missing page detection**

Referenced but not found on filesystem.

- [ ] **Step 3: Implement weak relation detection**

Pages with fewer than N links.

- [ ] **Step 4: Test**

- [ ] **Step 5: Commit**

---

### Task 17: Semantic Lint (LLM)

**Files:**
- Create: `gasket/engine/src/wiki/lint/semantic.rs`

- [ ] **Step 1: Implement contradiction detection**

LLM compares pages that reference the same entities for conflicting claims.

- [ ] **Step 2: Implement stale claim detection**

LLM checks for time-sensitive claims that may be outdated.

- [ ] **Step 3: Test**

- [ ] **Step 4: Commit**

---

### Task 18: WikiLintHook (Cron)

**Files:**
- Create: `gasket/engine/src/hooks/wiki_lint.rs`

- [ ] **Step 1: Implement periodic lint hook**

Runs on configurable interval (default 24h). Calls WikiLinter, auto-fixes simple issues.

- [ ] **Step 2: Register in HookBuilder**

- [ ] **Step 3: Test**

- [ ] **Step 4: Commit**

---

## Phase 5: CLI and Polish

### Task 19: CLI Commands

**Files:**
- Modify: `gasket/cli/src/commands/wiki.rs`

- [ ] **Step 1: Implement `wiki ingest <path>` CLI command**

- [ ] **Step 2: Implement `wiki search <query>` CLI command**

- [ ] **Step 3: Implement `wiki list` CLI command**

- [ ] **Step 4: Implement `wiki lint` CLI command**

- [ ] **Step 5: Commit**

---

### Task 20: Integration Tests

**Files:**
- Create: `gasket/engine/tests/wiki_integration.rs`

- [ ] **Step 1: Test full ingest → query → lint cycle**

- [ ] **Step 2: Test concurrent write safety**

- [ ] **Step 3: Test link consistency between frontmatter and SQLite**

- [ ] **Step 4: Commit**

---

### Task 21: Telemetry and Documentation

**Files:**
- Modify: `gasket/engine/src/wiki/mod.rs` (add OpenTelemetry metrics)
- Modify: `CLAUDE.md` (update docs)

- [ ] **Step 1: Add wiki.* OpenTelemetry metrics**

- [ ] **Step 2: Update CLAUDE.md with wiki architecture**

- [ ] **Step 3: Final commit**

---

## Dependency Order

```
Task 1 (Data Types) ─────────────────┐
Task 2 (SQLite Tables) ──────────────┤
                                      ├─→ Task 3 (PageStore+PageIndex+WikiLog)
                                      │         │
                                      │         └─→ Task 4 (Config+Session)
                                      │                    │
                                      │                    ├─→ Task 5 (Tools: keep names)
                                      │                    ├─→ Task 6 (Hooks)
                                      │                    └─→ Task 7 (Remove memory/)
                                      │                          │
                                      │                          ├─→ Task 8 (CLI + migrate)
                                      │                          │
                                      │                          ├─→ Phase 2 (Ingest: Tasks 9-13)
                                      │                          │   └─→ Task 11 (cost gate)
                                      │                          │
                                      │                          ├─→ Phase 3 (Query: Tasks 14-15)
                                      │                          │
                                      │                          ├─→ Phase 4 (Lint: Tasks 16-18)
                                      │                          │
                                      │                          └─→ Phase 5 (Polish: Tasks 19-21)
```

**Phase 1 (Tasks 1-8) is the critical path — CRUD only.** No Ingest, no Lint, no Tantivy in Phase 1.

**Phases 2-5 can partially overlap** once their Phase 1 dependencies are met.
