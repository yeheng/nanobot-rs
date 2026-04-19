# Wiki-First Knowledge System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing memory/ module with a Wiki-first knowledge management system using a three-layer architecture (Raw Sources, Compiled Wiki, Schema).

**Architecture:** The Wiki system stores knowledge as interlinked Markdown pages under `~/.gasket/wiki/` with SQLite metadata, Tantivy full-text search, and embedding-based semantic retrieval. A WikiStore facade provides CRUD, ingest, query, and lint operations. The existing memory/ module is cleanly removed.

**Tech Stack:** Rust, tokio, SQLite (sqlx), Tantivy, ONNX embeddings (fastembed), serde_yaml (frontmatter), chrono

**Spec:** `docs/specs/2026-04-19-wiki-first-knowledge-system-design.md`

---

## File Structure

### New Files (engine crate)

```
gasket/engine/src/wiki/
├── mod.rs                 # WikiStore facade + module re-exports
├── page.rs                # WikiPage, PageFrontmatter, PageType
├── uri.rs                 # WikiUri enum + parser + resolver
├── ingest/
│   ├── mod.rs             # IngestPipeline, IngestRequest, IngestReport
│   ├── parser.rs          # SourceParser trait + Markdown/HTML/Text parsers
│   ├── extractor.rs       # KnowledgeExtractor (LLM prompt + response parse)
│   ├── integrator.rs      # WikiIntegrator (LLM-driven page updates)
│   └── dedup.rs           # SemanticDeduplicator (embedding similarity)
├── query/
│   ├── mod.rs             # WikiQueryEngine (three-phase retrieval)
│   ├── tantivy_adapter.rs # Tantivy index adapter for wiki pages
│   └── reranker.rs        # Hybrid reranking (BM25 + embedding)
├── lint/
│   ├── mod.rs             # WikiLinter, LintReport
│   ├── structural.rs      # Structural checks (orphans, missing, weak)
│   └── semantic.rs        # Semantic checks (contradictions, stale)
├── index.rs               # index.md generation and maintenance
└── log.rs                 # log.md append + SQLite structured log

gasket/engine/src/hooks/
├── wiki_ingest.rs         # Replaces evolution.rs for Quick Ingest
└── wiki_query.rs          # Replaces memory loading in BeforeRequest
```

### New Files (storage crate)

```
gasket/storage/src/wiki/
├── mod.rs                 # Re-exports
├── tables.rs              # SQLite table creation (wiki_pages, raw_sources, etc.)
├── page_store.rs          # Wiki page metadata CRUD
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
gasket/engine/src/session/mod.rs            # WikiStore replaces MemoryManager
gasket/engine/src/session/config.rs         # WikiConfig replaces MemoryConfig
gasket/engine/src/session/history/builder.rs # WikiQueryEngine replaces MemoryLoader
gasket/engine/src/hooks/mod.rs              # Add wiki hooks
gasket/engine/src/hooks/evolution.rs        # Refactor to use WikiStore
gasket/engine/src/tools/mod.rs              # Replace memory tools with wiki tools
gasket/engine/src/tools/builder.rs          # Register wiki tools
gasket/engine/src/tools/memorize.rs         # Replace with WikiIngestTool
gasket/engine/src/tools/memory_search.rs    # Replace with WikiSearchTool
gasket/engine/src/tools/memory_refresh.rs   # Replace with WikiRefreshTool
gasket/cli/src/commands/mod.rs              # Add wiki commands
gasket/cli/src/commands/memory.rs           # Replace with wiki commands
gasket/storage/src/lib.rs                   # pub mod wiki; memory module kept for transition
```

### Deleted Files (Phase 1 end)

```
gasket/engine/src/session/memory/           # Entire directory
```

---

## Phase 1: Foundation

### Task 1: Wiki Data Types

**Files:**
- Create: `gasket/engine/src/wiki/mod.rs`
- Create: `gasket/engine/src/wiki/page.rs`
- Create: `gasket/engine/src/wiki/uri.rs`

- [ ] **Step 1: Create wiki URI module**

```rust
// gasket/engine/src/wiki/uri.rs

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// wiki:// URI scheme for identifying wiki resources
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WikiUri {
    /// A wiki page: wiki://entities/projects/gasket
    Page { path: String },
    /// A raw source reference: source://uploads/2026-04-19-paper
    Source { id: String },
    /// The content index page
    Index,
    /// The operation log
    Log,
}

impl WikiUri {
    pub fn page(path: impl Into<String>) -> Self {
        Self::Page { path: path.into() }
    }

    pub fn source(id: impl Into<String>) -> Self {
        Self::Source { id: id.into() }
    }

    /// Parse a wiki:// or source:// URI string
    pub fn parse(uri: &str) -> anyhow::Result<Self> {
        if uri == "wiki://index" {
            return Ok(Self::Index);
        }
        if uri == "wiki://log" {
            return Ok(Self::Log);
        }
        if let Some(path) = uri.strip_prefix("wiki://") {
            if path.is_empty() {
                anyhow::bail!("empty wiki path: {}", uri);
            }
            return Ok(Self::Page { path: path.to_string() });
        }
        if let Some(id) = uri.strip_prefix("source://") {
            if id.is_empty() {
                anyhow::bail!("empty source id: {}", uri);
            }
            return Ok(Self::Source { id: id.to_string() });
        }
        anyhow::bail!("invalid wiki URI: {}", uri)
    }

    /// Convert URI to filesystem path relative to wiki base directory
    pub fn to_file_path(&self, base: &Path) -> PathBuf {
        match self {
            Self::Page { path } => base.join(format!("{}.md", path)),
            Self::Source { id } => base.join("sources").join(format!("{}.md", id)),
            Self::Index => base.join("index.md"),
            Self::Log => base.join("log.md"),
        }
    }

    /// Convert to URI string
    pub fn as_str(&self) -> String {
        match self {
            Self::Page { path } => format!("wiki://{}", path),
            Self::Source { id } => format!("source://{}", id),
            Self::Index => "wiki://index".to_string(),
            Self::Log => "wiki://log".to_string(),
        }
    }
}

impl std::fmt::Display for WikiUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
```

- [ ] **Step 2: Create wiki page module**

```rust
// gasket/engine/src/wiki/page.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::uri::WikiUri;

/// Page type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageType {
    Entity,
    Topic,
    Source,
}

/// Relation types between wiki pages
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    References,
    Contradicts,
    Supersedes,
    Related,
}

/// YAML frontmatter for a wiki page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageFrontmatter {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub page_type: PageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub outgoing_links: Vec<String>,
    #[serde(default)]
    pub incoming_links: Vec<String>,
}

/// A complete wiki page (frontmatter + markdown body)
#[derive(Debug, Clone)]
pub struct WikiPage {
    pub id: WikiUri,
    pub frontmatter: PageFrontmatter,
    pub content: String,
}

impl WikiPage {
    /// Create a new entity page
    pub fn new_entity(title: impl Into<String>, category: impl Into<String>, content: impl Into<String>) -> Self {
        let title = title.into();
        let category = category.into();
        let slug = slugify(&title);
        let id = WikiUri::page(format!("entities/{}/{}", category, slug));
        let now = Utc::now();
        Self {
            id,
            frontmatter: PageFrontmatter {
                id: id.as_str(),
                title,
                page_type: PageType::Entity,
                category: Some(category),
                tags: vec![],
                created: now,
                updated: now,
                source_count: None,
                confidence: None,
                outgoing_links: vec![],
                incoming_links: vec![],
            },
            content: content.into(),
        }
    }

    /// Create a new topic page
    pub fn new_topic(title: impl Into<String>, content: impl Into<String>) -> Self {
        let title = title.into();
        let slug = slugify(&title);
        let id = WikiUri::page(format!("topics/{}", slug));
        let now = Utc::now();
        Self {
            id,
            frontmatter: PageFrontmatter {
                id: id.as_str(),
                title,
                page_type: PageType::Topic,
                category: None,
                tags: vec![],
                created: now,
                updated: now,
                source_count: None,
                confidence: None,
                outgoing_links: vec![],
                incoming_links: vec![],
            },
            content: content.into(),
        }
    }

    /// Create a new source summary page
    pub fn new_source(title: impl Into<String>, source_id: impl Into<String>, content: impl Into<String>) -> Self {
        let title = title.into();
        let source_id = source_id.into();
        let slug = slugify(&title);
        let id = WikiUri::page(format!("sources/{}", slug));
        let now = Utc::now();
        Self {
            id,
            frontmatter: PageFrontmatter {
                id: id.as_str(),
                title,
                page_type: PageType::Source,
                category: Some(source_id.clone()),
                tags: vec![],
                created: now,
                updated: now,
                source_count: Some(1),
                confidence: None,
                outgoing_links: vec![],
                incoming_links: vec![],
            },
            content: content.into(),
        }
    }

    /// Serialize to markdown with frontmatter
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&serde_yaml::to_string(&self.frontmatter).unwrap_or_default());
        out.push_str("---\n\n");
        out.push_str(&self.content);
        out
    }

    /// Deserialize from markdown with frontmatter
    pub fn from_markdown(id: WikiUri, markdown: &str) -> anyhow::Result<Self> {
        let content = markdown.trim_start();
        if !content.starts_with("---") {
            anyhow::bail!("missing frontmatter delimiter");
        }
        let rest = &content[3..];
        let end = rest.find("\n---").ok_or_else(|| anyhow::anyhow!("unclosed frontmatter"))?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').trim_start();

        let frontmatter: PageFrontmatter = serde_yaml::from_str(yaml)?;
        Ok(Self {
            id,
            frontmatter,
            content: body.to_string(),
        })
    }
}

/// Summary for index listing (no content)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub id: String,
    pub title: String,
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub updated: DateTime<Utc>,
    pub confidence: Option<f64>,
}

impl From<&PageFrontmatter> for PageSummary {
    fn from(fm: &PageFrontmatter) -> Self {
        Self {
            id: fm.id.clone(),
            title: fm.title.clone(),
            page_type: fm.page_type.clone(),
            category: fm.category.clone(),
            tags: fm.tags.clone(),
            updated: fm.updated,
            confidence: fm.confidence,
        }
    }
}

/// Filter for listing pages
#[derive(Debug, Clone, Default)]
pub struct PageFilter {
    pub page_type: Option<PageType>,
    pub category: Option<String>,
    pub tags: Vec<String>,
}

fn slugify(s: &str) -> String {
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

- [ ] **Step 3: Create wiki module root with tests**

```rust
// gasket/engine/src/wiki/mod.rs

pub mod page;
pub mod uri;

// Re-exports
pub use page::{PageFilter, PageFrontmatter, PageSummary, PageType, Relation, WikiPage};
pub use uri::WikiUri;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wiki_uri_parse_page() {
        let uri = WikiUri::parse("wiki://entities/projects/gasket").unwrap();
        assert_eq!(uri, WikiUri::page("entities/projects/gasket"));
    }

    #[test]
    fn test_wiki_uri_parse_source() {
        let uri = WikiUri::parse("source://uploads/2026-04-19-paper").unwrap();
        assert_eq!(uri, WikiUri::source("uploads/2026-04-19-paper"));
    }

    #[test]
    fn test_wiki_uri_parse_special() {
        assert_eq!(WikiUri::parse("wiki://index").unwrap(), WikiUri::Index);
        assert_eq!(WikiUri::parse("wiki://log").unwrap(), WikiUri::Log);
    }

    #[test]
    fn test_wiki_uri_to_file_path() {
        let base = std::path::Path::new("/tmp/wiki");
        let uri = WikiUri::page("entities/projects/gasket");
        assert_eq!(
            uri.to_file_path(base),
            std::path::PathBuf::from("/tmp/wiki/entities/projects/gasket.md")
        );
    }

    #[test]
    fn test_wiki_uri_roundtrip() {
        let uri = WikiUri::page("topics/rust-async");
        let s = uri.as_str();
        let parsed = WikiUri::parse(&s).unwrap();
        assert_eq!(uri, parsed);
    }

    #[test]
    fn test_wiki_page_entity_creation() {
        let page = WikiPage::new_entity("Gasket Project", "projects", "A Rust agent framework.");
        assert!(matches!(page.frontmatter.page_type, PageType::Entity));
        assert!(matches!(page.id, WikiUri::Page { .. }));
    }

    #[test]
    fn test_wiki_page_markdown_roundtrip() {
        let page = WikiPage::new_topic("Test Topic", "Some content here.");
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown(page.id.clone(), &md).unwrap();
        assert_eq!(parsed.frontmatter.title, "Test Topic");
        assert_eq!(parsed.content, "Some content here.");
    }

    #[test]
    fn test_wiki_uri_rejects_invalid() {
        assert!(WikiUri::parse("http://example.com").is_err());
        assert!(WikiUri::parse("wiki://").is_err());
    }
}
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test --package gasket-engine -- wiki::tests --nocapture`
Expected: All 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/wiki/
git commit -m "feat(wiki): add WikiUri, WikiPage, and PageFrontmatter data types"
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

/// Create all wiki-related tables
pub async fn create_wiki_tables(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_pages (
            id          TEXT PRIMARY KEY,
            path        TEXT NOT NULL UNIQUE,
            title       TEXT NOT NULL,
            type        TEXT NOT NULL,
            category    TEXT,
            tags        TEXT,
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

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_page_locks (
            page_id     TEXT PRIMARY KEY,
            locked_at   TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_type ON wiki_pages(type)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_category ON wiki_pages(category)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_updated ON wiki_pages(updated)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_raw_sources_ingested ON raw_sources(ingested)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_log_action ON wiki_log(action)")
        .execute(pool)
        .await?;

    Ok(())
}
```

- [ ] **Step 3: Create page store**

```rust
// gasket/storage/src/wiki/page_store.rs

use anyhow::Result;
use sqlx::SqlitePool;

/// SQLite-backed wiki page metadata store
pub struct WikiPageStore {
    pool: SqlitePool,
}

impl WikiPageStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(
        &self,
        id: &str,
        path: &str,
        title: &str,
        page_type: &str,
        category: Option<&str>,
        tags: &str,
        source_count: u32,
        confidence: f64,
        checksum: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO wiki_pages (id, path, title, type, category, tags, created, updated, source_count, confidence, checksum)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                path = excluded.path,
                title = excluded.title,
                type = excluded.type,
                category = excluded.category,
                tags = excluded.tags,
                updated = excluded.updated,
                source_count = excluded.source_count,
                confidence = excluded.confidence,
                checksum = excluded.checksum
            "#,
        )
        .bind(id).bind(path).bind(title).bind(page_type)
        .bind(category).bind(tags).bind(&now).bind(&now)
        .bind(source_count).bind(confidence).bind(checksum)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<PageRow>> {
        let row = sqlx::query_as::<_, PageRow>(
            "SELECT id, path, title, type, category, tags, created, updated, source_count, confidence, checksum FROM wiki_pages WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_pages WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_by_type(&self, page_type: &str) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT id, path, title, type, category, tags, created, updated, source_count, confidence, checksum FROM wiki_pages WHERE type = ? ORDER BY updated DESC"
        )
        .bind(page_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_all(&self) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT id, path, title, type, category, tags, created, updated, source_count, confidence, checksum FROM wiki_pages ORDER BY updated DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Acquire advisory lock for a page (auto-expires after 30s)
    pub async fn acquire_lock(&self, page_id: &str) -> Result<bool> {
        let now = chrono::Utc::now();
        let threshold = (now - chrono::Duration::seconds(30)).to_rfc3339();

        // Clean expired locks
        sqlx::query("DELETE FROM wiki_page_locks WHERE locked_at < ?")
            .bind(&threshold)
            .execute(&self.pool)
            .await?;

        // Try to acquire
        let result = sqlx::query(
            "INSERT OR IGNORE INTO wiki_page_locks (page_id, locked_at) VALUES (?, ?)"
        )
        .bind(page_id)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Release advisory lock for a page
    pub async fn release_lock(&self, page_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_page_locks WHERE page_id = ?")
            .bind(page_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct PageRow {
    pub id: String,
    pub path: String,
    pub title: String,
    #[sqlx(rename = "type")]
    pub page_type: String,
    pub category: Option<String>,
    pub tags: Option<String>,
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

### Task 3: WikiStore Facade

**Files:**
- Modify: `gasket/engine/src/wiki/mod.rs` (add WikiStore struct)

- [ ] **Step 1: Write WikiStore facade**

```rust
// Add to gasket/engine/src/wiki/mod.rs

use anyhow::Result;
use gasket_storage::wiki::{WikiPageStore, WikiLogStore, WikiRelationStore, WikiSourceStore};
use std::path::{Path, PathBuf};
use tokio::fs;

/// The core wiki store - facade over filesystem + SQLite
pub struct WikiStore {
    wiki_base: PathBuf,
    sources_base: PathBuf,
    page_store: WikiPageStore,
    log_store: WikiLogStore,
    relation_store: WikiRelationStore,
    source_store: WikiSourceStore,
}

impl WikiStore {
    pub fn new(
        wiki_base: PathBuf,
        sources_base: PathBuf,
        pool: sqlx::SqlitePool,
    ) -> Self {
        Self {
            wiki_base,
            sources_base,
            page_store: WikiPageStore::new(pool.clone()),
            log_store: WikiLogStore::new(pool.clone()),
            relation_store: WikiRelationStore::new(pool.clone()),
            source_store: WikiSourceStore::new(pool),
        }
    }

    /// Ensure wiki directory structure exists
    pub async fn init_dirs(&self) -> Result<()> {
        for dir in &["entities/people", "entities/projects", "entities/concepts", "topics", "sources"] {
            fs::create_dir_all(self.wiki_base.join(dir)).await?;
        }
        fs::create_dir_all(self.sources_base.join("uploads")).await?;
        fs::create_dir_all(self.sources_base.join("web")).await?;
        fs::create_dir_all(self.sources_base.join("conversations")).await?;
        fs::create_dir_all(self.sources_base.join("channels")).await?;
        Ok(())
    }

    // --- CRUD ---

    pub async fn read_page(&self, id: &WikiUri) -> Result<WikiPage> {
        let path = id.to_file_path(&self.wiki_base);
        let markdown = fs::read_to_string(&path).await?;
        WikiPage::from_markdown(id.clone(), &markdown)
    }

    pub async fn write_page(&self, id: &WikiUri, page: &WikiPage) -> Result<()> {
        let path = id.to_file_path(&self.wiki_base);

        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Acquire lock
        let locked = self.page_store.acquire_lock(&id.as_str()).await?;
        if !locked {
            anyhow::bail!("failed to acquire lock for page: {}", id);
        }

        // Write filesystem
        fs::write(&path, page.to_markdown()).await?;

        // Update SQLite metadata
        let tags_str = serde_json::to_string(&page.frontmatter.tags)?;
        self.page_store.upsert(
            &page.frontmatter.id,
            path.strip_prefix(&self.wiki_base)?.to_str().unwrap_or(""),
            &page.frontmatter.title,
            match &page.frontmatter.page_type { PageType::Entity => "entity", PageType::Topic => "topic", PageType::Source => "source" },
            page.frontmatter.category.as_deref(),
            &tags_str,
            page.frontmatter.source_count.unwrap_or(0),
            page.frontmatter.confidence.unwrap_or(1.0),
            None,
        ).await?;

        // Release lock
        self.page_store.release_lock(&id.as_str()).await?;
        Ok(())
    }

    pub async fn delete_page(&self, id: &WikiUri) -> Result<()> {
        let path = id.to_file_path(&self.wiki_base);
        fs::remove_file(&path).await?;
        self.page_store.delete(&id.as_str()).await?;
        Ok(())
    }

    pub async fn list_pages(&self, filter: PageFilter) -> Result<Vec<PageSummary>> {
        let rows = match &filter.page_type {
            Some(pt) => self.page_store.list_by_type(match pt { PageType::Entity => "entity", PageType::Topic => "topic", PageType::Source => "source" }).await?,
            None => self.page_store.list_all().await?,
        };
        Ok(rows.iter().map(|r| PageSummary {
            id: r.id.clone(),
            title: r.title.clone(),
            page_type: match r.page_type.as_str() { "entity" => PageType::Entity, "topic" => PageType::Topic, _ => PageType::Source },
            category: r.category.clone(),
            tags: r.tags.as_ref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default(),
            updated: chrono::DateTime::parse_from_rfc3339(&r.updated).map(|dt| dt.with_timezone(&chrono::Utc)).unwrap_or_default(),
            confidence: Some(r.confidence),
        }).collect())
    }

    // --- Relations ---
    pub async fn add_relation(&self, from: &WikiUri, to: &WikiUri, relation: &str) -> Result<()> {
        self.relation_store.add(&from.as_str(), &to.as_str(), relation).await
    }
    pub async fn get_relations(&self, id: &WikiUri) -> Result<Vec<(String, String, String)>> {
        self.relation_store.get_outgoing(&id.as_str()).await
    }

    // --- Sources ---
    pub async fn register_source(&self, id: &str, path: &str, format: &str, title: &str) -> Result<()> {
        self.source_store.register(id, path, format, title).await
    }

    // --- Index & Log ---
    pub async fn rebuild_index(&self) -> Result<()> {
        // TODO: Phase 2 - generate index.md from all pages
        Ok(())
    }
    pub async fn append_log(&self, entry: &str) -> Result<()> {
        let log_path = self.wiki_base.join("log.md");
        let existing = fs::read_to_string(&log_path).await.unwrap_or_default();
        fs::write(&log_path, format!("{}{}", existing, entry)).await?;
        Ok(())
    }
    pub async fn log_to_db(&self, action: &str, target: &str, detail: &str) -> Result<()> {
        self.log_store.append(action, target, detail).await
    }
}
```

- [ ] **Step 2: Add wiki module to engine lib.rs**

Add `pub mod wiki;` to `gasket/engine/src/lib.rs` and add necessary imports.

- [ ] **Step 3: Run build**

Run: `cargo build --package gasket-engine`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/wiki/mod.rs gasket/engine/src/lib.rs
git commit -m "feat(wiki): add WikiStore facade with CRUD, relations, and locking"
```

---

### Task 4: WikiConfig and Session Integration

**Files:**
- Modify: `gasket/engine/src/session/config.rs` (add WikiConfig, remove MemoryConfig references)
- Modify: `gasket/engine/src/session/mod.rs` (WikiStore replaces MemoryManager)

- [ ] **Step 1: Add WikiConfig to session config**

Add to `gasket/engine/src/session/config.rs`:

```rust
/// Wiki system configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WikiConfig {
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
```

- [ ] **Step 2: Replace MemoryManager field with WikiStore in session**

In `gasket/engine/src/session/mod.rs`:
- Change `memory_manager: Option<Arc<MemoryManager>>` to `wiki_store: Option<Arc<WikiStore>>`
- Update all methods that use `memory_manager` to use `wiki_store`
- Update `try_init_memory_manager` → `try_init_wiki_store`

- [ ] **Step 3: Build and fix compile errors**

Run: `cargo build --package gasket-engine 2>&1 | head -50`
Expected: Compile errors in tools/ and hooks/ that reference old memory types (fixed in Task 5)

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/session/
git commit -m "refactor(session): replace MemoryManager with WikiStore in session config"
```

---

### Task 5: Replace Memory Tools with Wiki Tools

**Files:**
- Modify: `gasket/engine/src/tools/memorize.rs` → rewrite as `WikiIngestTool`
- Modify: `gasket/engine/src/tools/memory_search.rs` → rewrite as `WikiSearchTool`
- Modify: `gasket/engine/src/tools/memory_refresh.rs` → rewrite as `WikiRefreshTool`
- Modify: `gasket/engine/src/tools/builder.rs` (register wiki tools)
- Modify: `gasket/engine/src/tools/mod.rs` (update exports)

- [ ] **Step 1: Rewrite memorize.rs as WikiIngestTool**

Replace the `MemorizeTool` with `WikiIngestTool` that creates wiki pages. Keep the same Tool trait interface. Key changes:
- `memorize` → `wiki_ingest`
- Creates a WikiPage and writes via WikiStore
- Updates index after creation

- [ ] **Step 2: Rewrite memory_search.rs as WikiSearchTool**

Replace `MemorySearchTool` with `WikiSearchTool`. Initially uses SQLite metadata search (Tantivy integration in Phase 3). Key changes:
- `memory_search` → `wiki_search`
- Queries WikiPageStore for matching pages
- Returns page summaries

- [ ] **Step 3: Rewrite memory_refresh.rs as WikiRefreshTool**

Replace `MemoryRefreshTool` with `WikiRefreshTool`:
- `memory_refresh` → `wiki_refresh`
- Triggers `rebuild_index()` on WikiStore

- [ ] **Step 4: Update tool builder registration**

In `gasket/engine/src/tools/builder.rs`:
- Replace `MemorizeTool` registration with `WikiIngestTool`
- Replace `MemorySearchTool` with `WikiSearchTool`
- Replace `MemoryRefreshTool` with `WikiRefreshTool`

- [ ] **Step 5: Update tools/mod.rs exports**

- [ ] **Step 6: Build and verify**

Run: `cargo build --package gasket-engine`
Expected: Compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/tools/
git commit -m "refactor(tools): replace memory tools with wiki tools (ingest/search/refresh)"
```

---

### Task 6: Update Hooks

**Files:**
- Modify: `gasket/engine/src/hooks/evolution.rs` (use WikiStore)
- Create: `gasket/engine/src/hooks/wiki_query.rs` (replace MemoryLoader hook)
- Modify: `gasket/engine/src/hooks/mod.rs` (add new hook)

- [ ] **Step 1: Refactor EvolutionHook to use WikiStore**

Change `evolution.rs` to:
- Accept `Arc<WikiStore>` instead of `Arc<MemoryManager>`
- Convert extracted knowledge items to `IngestRequest`
- Call `WikiStore::write_page()` instead of `MemoryManager::create_memory()`

- [ ] **Step 2: Create WikiQueryHook**

```rust
// gasket/engine/src/hooks/wiki_query.rs
// BeforeRequest hook that loads relevant wiki pages into context
```

- [ ] **Step 3: Register hooks and build**

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/hooks/
git commit -m "refactor(hooks): update EvolutionHook for WikiStore, add WikiQueryHook"
```

---

### Task 7: Remove Memory Module

**Files:**
- Delete: `gasket/engine/src/session/memory/` (entire directory)
- Modify: `gasket/engine/src/session/mod.rs` (remove `pub mod memory;`)
- Modify: `gasket/engine/src/lib.rs` (remove memory re-exports)
- Verify: `gasket/engine/src/session/history/builder.rs` (no memory references remain)

- [ ] **Step 1: Search for remaining memory references**

Run: `grep -rn "memory::" gasket/engine/src/ --include="*.rs"`
Expected: Zero references to `session::memory`

- [ ] **Step 2: Delete memory directory**

```bash
rm -rf gasket/engine/src/session/memory/
```

- [ ] **Step 3: Remove memory module declaration**

Remove `pub mod memory;` from `session/mod.rs` and remove `MemoryManager`/`MemoryContext` re-exports.

- [ ] **Step 4: Fix remaining compile errors**

Search for any `MemoryManager`, `MemoryLoader`, `MemoryWriter`, `MemoryContext` references and replace with wiki equivalents.

- [ ] **Step 5: Build**

Run: `cargo build --package gasket-engine`
Expected: Clean build, no errors

- [ ] **Step 6: Run tests**

Run: `cargo test --package gasket-engine`
Expected: All tests pass (memory-specific tests are gone, wiki tests pass)

- [ ] **Step 7: Commit**

```bash
git add -A gasket/engine/src/
git commit -m "refactor(engine): remove memory/ module, fully replaced by wiki/"
```

---

### Task 8: Update CLI Commands

**Files:**
- Modify: `gasket/cli/src/commands/memory.rs` → rewrite as `wiki.rs`
- Modify: `gasket/cli/src/commands/mod.rs` (add wiki commands)
- Modify: `gasket/cli/src/main.rs` (update CLI dispatch)

- [ ] **Step 1: Rewrite CLI memory commands as wiki commands**

Replace `memory refresh` / `memory decay` with:
- `wiki ingest <path>` - Import a document
- `wiki search <query>` - Search wiki pages
- `wiki list` - List all wiki pages
- `wiki lint` - Run health check

- [ ] **Step 2: Update CLI dispatch in main.rs**

- [ ] **Step 3: Build CLI**

Run: `cargo build --package gasket-cli`
Expected: Clean build (ignoring pre-existing tui.rs errors)

- [ ] **Step 4: Commit**

```bash
git add gasket/cli/src/
git commit -m "refactor(cli): replace memory commands with wiki commands"
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

### Task 11: Wiki Integrator (LLM-driven)

**Files:**
- Create: `gasket/engine/src/wiki/ingest/integrator.rs`

- [ ] **Step 1: Implement WikiIntegrator**

For Deep Ingest: analyze impact on existing pages, update affected pages, extract relations.
For Quick Ingest: create single page + update index.

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

Per spec Section 10: id, path, title, content, summary, type, category, tags, created, updated, confidence, source_count.

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
                                      ├─→ Task 3 (WikiStore Facade) ─→ Task 4 (Config+Session)
                                      │                                    │
                                      │                                    ├─→ Task 5 (Tools)
                                      │                                    ├─→ Task 6 (Hooks)
                                      │                                    └─→ Task 7 (Remove Memory)
                                      │                                          │
                                      │                                          ├─→ Task 8 (CLI)
                                      │                                          │
                                      │                                          ├─→ Phase 2 (Ingest: Tasks 9-13)
                                      │                                          │
                                      │                                          ├─→ Phase 3 (Query: Tasks 14-15)
                                      │                                          │
                                      │                                          ├─→ Phase 4 (Lint: Tasks 16-18)
                                      │                                          │
                                      │                                          └─→ Phase 5 (Polish: Tasks 19-21)
```

**Phase 1 (Tasks 1-8) is the critical path.** All subsequent phases depend on Phase 1 completing first.

**Phases 2-5 can partially overlap** once their Phase 1 dependencies are met (e.g., Task 9 can start once Task 3 is done).
