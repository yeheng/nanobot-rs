# Wiki-First Knowledge System Design

**Date:** 2026-04-19
**Author:** Claude + yeheng
**Status:** Draft
**Inspired by:** [Karpathy's LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)

---

## 1. Executive Summary

Replace Gasket's existing `memory/` module with a Wiki-first knowledge management system inspired by Karpathy's LLM Wiki pattern. The core insight: **knowledge should be compiled once and kept current, not re-derived on every query.**

The Wiki system introduces three layers (Raw Sources, Compiled Wiki, Schema) and three operations (Ingest, Query, Lint) to create a persistent, compounding knowledge artifact that grows richer with every source added and every question asked.

**Scope:**
- Agent self-evolution (learning from conversations)
- User personal knowledge management (document import)
- Agent exploratory learning (task-driven knowledge capture)
- Multi-channel message integration (Telegram/Discord/Slack)

**Scale target:** 2000+ pages, using Tantivy + FTS5 + embedding hybrid retrieval.

---

## 2. Architecture Overview

### 2.1 Three-Layer Model

```
+----------------------------------------------+
|  Schema Layer (AgentConfig / CLAUDE.md)      |  <- Behavior specification
+----------------------------------------------+
|  Compiled Wiki (~/.gasket/wiki/)             |  <- LLM-maintained Markdown pages
|  +-- index.md (content catalog)              |
|  +-- log.md (append-only operation log)       |
|  +-- entities/ (people, projects, concepts)  |
|  +-- topics/ (syntheses, comparisons, guides) |
|  +-- sources/ (source summary pages)          |
+----------------------------------------------+
|  Raw Sources (~/.gasket/sources/)            |  <- Immutable source documents
|  +-- uploads/ (PDF, MD, images)              |
|  +-- web/ (web page snapshots)               |
|  +-- conversations/ (conversation archives)  |
|  +-- channels/ (channel message archives)    |
+----------------------------------------------+
```

### 2.2 Directory Layout

```
~/.gasket/
+-- config.yaml                     # Extended with wiki: section
+-- wiki/                           # Compiled Wiki layer
|   +-- index.md                    # Auto-maintained content catalog
|   +-- log.md                      # Append-only operation log
|   +-- entities/
|   |   +-- people/
|   |   +-- projects/
|   |   +-- concepts/
|   +-- topics/
|   +-- sources/
+-- sources/                        # Raw Sources layer (immutable)
|   +-- uploads/
|   +-- web/
|   +-- conversations/
|   +-- channels/
|       +-- telegram/
|       +-- discord/
+-- gasket.db                       # SQLite (metadata + embeddings + relations)
```

---

## 3. Data Model

### 3.1 Wiki Page Format

**SQLite is the single source of truth.** Markdown files on disk are a derived cache, written from SQLite and regenerated on demand. This eliminates the dual-truth bug class entirely.

Every wiki page is stored as a SQLite row with a `content` column. Markdown files are optionally written to disk for human readability and git-friendliness, but they are never read back as authoritative data.

**SQLite row (authoritative):**

| Column | Type | Description |
|--------|------|-------------|
| `path` | TEXT PK | Relative path: `entities/projects/gasket` |
| `title` | TEXT | "Gasket Project" |
| `type` | TEXT | `entity` \| `topic` \| `source` |
| `category` | TEXT | Optional subcategory |
| `tags` | TEXT | JSON array |
| `content` | TEXT | Full markdown body |
| `created` | TEXT | ISO 8601 |
| `updated` | TEXT | ISO 8601 |
| `source_count` | INTEGER | Number of sources that contributed |
| `confidence` | REAL | 0.0-1.0 |
| `checksum` | TEXT | Content hash for change detection |

**Derived markdown file (cache, at `~/.gasket/wiki/entities/projects/gasket.md`):**

```markdown
---
title: "Gasket Project"
type: entity
category: project
tags: [rust, agent, llm, mcp]
updated: 2026-04-19T15:30:00Z
---

# Gasket Project

## Overview
Gasket is a Rust LLM Agent framework...

## Key Decisions
- 2026-03: Chose Actor Model as core architecture...

## Related
- [[actor-model]] - Core architecture pattern
- [[yeheng]] - Project lead
```

**Key simplification:** No custom URI scheme (`wiki://`). Page identity is the `path` string (e.g., `"entities/projects/gasket"`). This is just a relative filesystem path — no parser needed, no edge cases.

### 3.2 Page Identity (Path-Based)

Pages are identified by their relative path string under the wiki root:

```
"entities/projects/gasket"        -> Page about Gasket project
"entities/people/yeheng"          -> Page about a person
"topics/architecture-overview"    -> Topic page
"sources/2026-04-19-karpathy"     -> Source summary page
"_index"                          -> Index page (underscore-prefixed = system)
"_log"                            -> Log page
```

Resolution: `PathBuf::from(wiki_root).join(format!("{}.md", path))`. No URI parser, no custom scheme.

### 3.3 SQLite Schema

```sql
-- Wiki pages: SINGLE SOURCE OF TRUTH. Content lives here.
-- Markdown files on disk are derived cache.
CREATE TABLE wiki_pages (
    path        TEXT PRIMARY KEY,       -- "entities/projects/gasket"
    title       TEXT NOT NULL,
    type        TEXT NOT NULL,           -- entity | topic | source
    category    TEXT,
    tags        TEXT,                    -- JSON array
    content     TEXT NOT NULL DEFAULT '',-- Full markdown body
    created     TEXT NOT NULL,
    updated     TEXT NOT NULL,
    source_count INTEGER DEFAULT 0,
    confidence  REAL DEFAULT 1.0,
    checksum    TEXT                     -- Content hash for cache invalidation
);

-- Raw source registry (ingestion queue)
CREATE TABLE raw_sources (
    id          TEXT PRIMARY KEY,
    path        TEXT NOT NULL,
    format      TEXT NOT NULL,           -- pdf | md | html | url | conversation | channel
    ingested    INTEGER DEFAULT 0,
    ingested_at TEXT,
    title       TEXT,
    metadata    TEXT,                    -- JSON
    created     TEXT NOT NULL
);

-- Inter-page relations (graph edges)
CREATE TABLE wiki_relations (
    from_page   TEXT NOT NULL REFERENCES wiki_pages(path),
    to_page     TEXT NOT NULL REFERENCES wiki_pages(path),
    relation    TEXT NOT NULL,           -- references | contradicts | supersedes | related
    confidence  REAL DEFAULT 1.0,
    created     TEXT NOT NULL,
    PRIMARY KEY (from_page, to_page, relation)
);

-- Structured operation log
CREATE TABLE wiki_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    action      TEXT NOT NULL,           -- ingest | query | lint | create | update | delete
    target      TEXT,
    detail      TEXT,                    -- JSON
    created     TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Existing tables retained:
-- session_events, session_summaries, session_embeddings, cron_state
```

**Key change from original design:** `wiki_pages.path` is now the PRIMARY KEY (not a separate `id` column). The `content` column lives in SQLite, not on disk. This makes SQLite the canonical data store and eliminates all sync problems.

---

## 4. Core Operations

### 4.1 Ingest Pipeline

Ingest converts raw information into structured wiki pages.

**Two-tier strategy:**
- **Quick Ingest** (1 page per item): Used for conversation extraction and agent exploration. Low LLM cost. Creates one page + updates index.
- **Deep Ingest** (up to 15 pages per source): Used for explicit file/document import. Higher LLM cost. Full integration with entity/topic cross-referencing. **Requires cost validation gate before execution.**

**Deep Ingest cost validation gate:**

Before executing a Deep Ingest, the system estimates LLM token consumption and compares against a configurable budget:

```rust
impl WikiIntegrator {
    /// Estimate token cost for a deep ingest before executing it
    pub fn estimate_cost(&self, source: &ParsedSource, wiki: &PageStore) -> CostEstimate {
        let source_tokens = estimate_tokens(&source.content);
        let affected_pages = wiki.list_relevant_pages(&source.entities);
        let update_tokens = affected_pages.len() as u32 * 2000; // ~2k tokens per page update
        let relation_tokens = affected_pages.len() as u32 * 500; // ~500 tokens per relation extraction

        CostEstimate {
            estimated_input_tokens: source_tokens + update_tokens + relation_tokens,
            estimated_pages_affected: affected_pages.len(),
            estimated_cost_usd: self.pricing.calculate(&self.model, estimated_input_tokens),
        }
    }

    pub async fn deep_ingest(&self, source: &ParsedSource, wiki: &PageStore) -> Result<IngestReport> {
        // Gate: validate cost before proceeding
        let estimate = self.estimate_cost(source, wiki);
        if estimate.estimated_cost_usd > self.max_cost_per_ingest {
            return Err(anyhow::anyhow!(
                "Deep ingest estimated cost ${:.4} exceeds budget ${:.4}. \
                 Affected pages: {}. Use quick_ingest instead or increase budget.",
                estimate.estimated_cost_usd, self.max_cost_per_ingest,
                estimate.estimated_pages_affected
            ));
        }
        // ... proceed with ingest
    }
}
```

**Configuration:**
```yaml
wiki:
  ingest:
    max_cost_per_ingest: 0.10  # USD, default 10 cents
    cost_warning_threshold: 0.05  # Warn but proceed above this
```

```
Source Ingestion (Phase 1) -> Wiki Integration (Phase 2) -> Index & Log (Phase 3)
```

**Phase 1: Source Ingestion**

```rust
pub struct IngestPipeline {
    parsers: Vec<Box<dyn SourceParser>>,
    extractor: KnowledgeExtractor,
    dedup: SemanticDeduplicator,
}

#[async_trait]
pub trait SourceParser: Send + Sync {
    fn format(&self) -> SourceFormat;
    async fn parse(&self, path: &Path) -> Result<ParsedSource>;
}

pub enum SourceFormat {
    Pdf, Markdown, Html, Url, Conversation, ChannelMessage,
}

pub struct ParsedSource {
    pub title: String,
    pub content: String,
    pub metadata: SourceMetadata,
    pub entities: Vec<ExtractedEntity>,
}
```

**Source-specific ingest triggers:**

| Source | Parser | Trigger | Tier |
|--------|--------|---------|------|
| Conversation history | EvolutionHook (refactored) | AfterResponse hook | Quick |
| File import (PDF/MD/URL) | Format-specific parser | User command `/ingest <path>` | Deep |
| Agent exploration | Tool result analysis | AfterToolCall hook | Quick |
| Channel messages | Channel archive + batch | Outbound hook, time-windowed | Quick |

**Phase 2: Wiki Integration (LLM-driven)**

```rust
pub struct WikiIntegrator {
    llm_client: LlmClient,
    page_resolver: PageResolver,
    relation_builder: RelationBuilder,
}

impl WikiIntegrator {
    pub async fn integrate(&self, source: &ParsedSource, wiki: &WikiStore) -> Result<IngestReport> {
        // 1. LLM analyzes impact: which existing pages does this source affect?
        let affected_pages = self.llm_analyze_impact(source, wiki.index()).await?;

        // 2. Create/update source summary page
        let source_page = wiki.create_source_page(source).await?;

        // 3. Update affected entity/topic pages
        for page_id in &affected_pages {
            let page = wiki.read_page(page_id).await?;
            let updated = self.llm_update_page(&page, source, &source_page).await?;
            wiki.write_page(page_id, &updated).await?;
        }

        // 4. Extract and create inter-page relations
        let relations = self.llm_extract_relations(source, &affected_pages).await?;
        wiki.write_relations(&relations).await?;

        // 5. Rebuild index
        wiki.rebuild_index().await?;

        Ok(IngestReport { source_page, affected_pages, relations })
    }
}
```

**Phase 3: Index & Log**

```rust
pub async fn update_index_and_log(wiki: &WikiStore, report: &IngestReport) -> Result<()> {
    // Rebuild index.md (content catalog by category)
    wiki.rebuild_index().await?;

    // Append to log.md (parseable format)
    let entry = format!(
        "## [{}] ingest | {} | +{} pages | +{} relations\n",
        chrono::now().format("%Y-%m-%d"),
        report.source_page.title,
        report.affected_pages.len(),
        report.relations.len(),
    );
    wiki.append_log(&entry).await?;

    // Structured log to SQLite
    wiki.log_to_db(IngestAction::from(report)).await?;

    Ok(())
}
```

### 4.2 Query Pipeline

Query replaces the existing MemoryLoader with unified wiki retrieval from SQLite.

```rust
pub struct WikiQueryEngine {
    index: PageIndex,
    store: PageStore,
}

impl WikiQueryEngine {
    pub async fn query(&self, query: &str, budget: TokenBudget) -> Result<QueryResult> {
        // Phase 1: Candidate retrieval (Tantivy hybrid search)
        let candidates = self.index.search(query, 50).await?;

        // Phase 2: Budget-aware selection
        let selected = self.apply_budget(&candidates, budget);

        // Phase 3: Load full page content from SQLite (not disk)
        let mut pages = Vec::with_capacity(selected.len());
        for summary in &selected {
            if let Ok(page) = self.store.read(&summary.path).await {
                pages.push(page);
            }
        }

        Ok(QueryResult { pages, total_candidates: candidates.len() })
    }

    /// File a good answer back into the wiki as a new topic page
    pub async fn file_answer(&self, question: &str, answer: &str, store: &PageStore) -> Result<()> {
        let path = format!("topics/{}", slugify(question));
        let page = WikiPage::new(path, question.to_string(), PageType::Topic, answer.to_string());
        store.write(&page).await?;
        Ok(())
    }
}
```

### 4.3 Lint Pipeline

Lint is a new capability for wiki health checking. Operates directly on SQLite data.

```rust
pub struct WikiLinter {
    llm_client: LlmClient,
}

pub struct LintReport {
    pub contradictions: Vec<Contradiction>,
    pub stale_claims: Vec<StaleClaim>,
    pub orphans: Vec<OrphanPage>,
    pub missing_pages: Vec<MissingPage>,
    pub weak_relations: Vec<WeakRelation>,
}

impl WikiLinter {
    pub async fn lint(&self, store: &PageStore) -> Result<LintReport> {
        let all_pages = store.list(PageFilter::default()).await?;
        let all_relations = store.list_all_relations().await?;

        // Structural checks (fast, no LLM) — pure SQL
        let orphans = Self::find_orphans(&all_pages, &all_relations)?;
        let missing_pages = Self::find_missing_pages(&all_pages, &all_relations)?;
        let weak_relations = Self::find_weak_relations(&all_relations)?;

        // Semantic checks (slow, LLM-driven) — only if semantic_checks enabled
        let (contradictions, stale_claims) = if self.semantic_enabled {
            let c = self.llm_find_contradictions(&all_pages).await?;
            let s = self.llm_find_stale_claims(&all_pages).await?;
            (c, s)
        } else {
            (vec![], vec![])
        };

        let report = LintReport { contradictions, stale_claims, orphans, missing_pages, weak_relations };
        Ok(report)
    }

    pub async fn auto_fix(&self, report: &LintReport, store: &PageStore) -> Result<FixReport> {
        // Auto-fix: create missing page stubs, link orphans
        // ...
    }
}
```
```

---

## 5. Module Architecture

### 5.1 Rust Module Structure

```
gasket/engine/src/
+-- wiki/                          # Wiki module (replaces memory/)
|   +-- mod.rs                     # Re-exports
|   +-- page.rs                    # WikiPage data model (one struct, one new())
|   +-- store.rs                   # PageStore: CRUD + disk sync
|   +-- index.rs                   # PageIndex: search (Tantivy + FTS5)
|   +-- log.rs                     # WikiLog: structured operation log
|   +-- ingest/
|   |   +-- mod.rs                 # IngestPipeline
|   |   +-- parser.rs              # SourceParser trait + implementations
|   |   +-- extractor.rs           # LLM knowledge extraction
|   |   +-- integrator.rs          # WikiIntegrator
|   |   +-- dedup.rs               # Semantic deduplication
|   +-- query/
|   |   +-- mod.rs                 # WikiQueryEngine
|   |   +-- tantivy_adapter.rs     # Tantivy search adapter
|   |   +-- reranker.rs            # Hybrid reranking
|   +-- lint/
|   |   +-- mod.rs                 # WikiLinter
|   |   +-- structural.rs          # Structural checks (fast)
|   |   +-- semantic.rs            # Semantic checks (LLM)
|
+-- hooks/
|   +-- evolution.rs               # Refactored: outputs to Wiki Ingest
|   +-- wiki_query.rs              # NEW: replaces MemoryLoader
|   +-- wiki_explorer.rs           # NEW: agent exploratory learning
|   +-- channel_archive.rs         # NEW: channel message archiving
|   +-- wiki_lint.rs               # NEW: periodic Lint (cron)
|
+-- session/
|   +-- config.rs                  # Extended with WikiConfig, MemoryConfig removed
|   +-- ...                        # memory/ submodule removed
|
+-- lib.rs                         # pub mod wiki; memory refs removed
```

**Simplification from original design:**
- Removed `uri.rs` — pages identified by `String` path, no custom URI scheme
- Split God Object `WikiStore` into three focused structs: `PageStore`, `PageIndex`, `WikiLog`
- Each struct owns one responsibility, one `SqlitePool` reference
- No `index.md`/`log.md` file maintenance — data lives in SQLite, files are optional export

### 5.2 Key Interfaces

```rust
// wiki/page.rs — One data struct, one constructor

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPage {
    pub path: String,               // "entities/projects/gasket"
    pub title: String,
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub content: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub source_count: u32,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PageType { Entity, Topic, Source }

impl WikiPage {
    /// One constructor. No special cases.
    pub fn new(path: String, title: String, page_type: PageType, content: String) -> Self {
        let now = Utc::now();
        Self {
            path, title, page_type, content,
            category: None,
            tags: vec![],
            created: now,
            updated: now,
            source_count: 0,
            confidence: 1.0,
        }
    }
}

// wiki/store.rs — PageStore: CRUD + disk sync

pub struct PageStore {
    pool: SqlitePool,
    wiki_root: PathBuf,
}

impl PageStore {
    pub async fn read(&self, path: &str) -> Result<WikiPage>;
    pub async fn write(&self, page: &WikiPage) -> Result<()>;
    pub async fn delete(&self, path: &str) -> Result<()>;
    pub async fn list(&self, filter: PageFilter) -> Result<Vec<PageSummary>>;
    pub async fn exists(&self, path: &str) -> Result<bool>;

    /// Sync a page to disk as markdown (optional, for human readability)
    pub async fn sync_to_disk(&self, page: &WikiPage) -> Result<()>;

    /// Rebuild disk cache from SQLite (after migration or corruption)
    pub async fn rebuild_disk_cache(&self) -> Result<usize>;
}

// wiki/index.rs — PageIndex: search

pub struct PageIndex {
    pool: SqlitePool,
    tantivy: TantivyIndex,
}

impl PageIndex {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<PageSummary>>;
    pub async fn upsert_document(&self, page: &WikiPage) -> Result<()>;
    pub async fn delete_document(&self, path: &str) -> Result<()>;
    pub async fn rebuild(&self, store: &PageStore) -> Result<()>;
}

// wiki/log.rs — WikiLog: audit trail

pub struct WikiLog {
    pool: SqlitePool,
}

impl WikiLog {
    pub async fn append(&self, action: &str, target: &str, detail: &str) -> Result<()>;
    pub async fn list_recent(&self, limit: usize) -> Result<Vec<LogEntry>>;
}

// Relations (in PageStore)

impl PageStore {
    pub async fn add_relation(&self, from: &str, to: &str, relation: &str) -> Result<()>;
    pub async fn get_outgoing(&self, path: &str) -> Result<Vec<RelationRow>>;
    pub async fn get_incoming(&self, path: &str) -> Result<Vec<RelationRow>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Relation { References, Contradicts, Supersedes, Related }
```

**Key differences from original design:**

| Original | Revised | Why |
|----------|---------|-----|
| `WikiUri` enum with custom URI parser | `String` path + `PathBuf` for resolution | YAGNI — PathBuf already does everything |
| `WikiStore` God Object (7+ responsibilities) | 3 structs: `PageStore`, `PageIndex`, `WikiLog` | Single responsibility |
| `new_entity()` / `new_topic()` / `new_source()` | One `new(path, title, type, content)` | No special cases |
| Frontmatter with `outgoing_links`/`incoming_links` | Relations in separate table, no frontmatter duplication | Single truth source |
| `index.md` / `log.md` file maintenance | Data in SQLite, files are optional export | No sync problems |

### 5.3 Hook Integration

```
Hook Pipeline (updated):

BeforeRequest
  +-- WikiQueryHook           <- Replaces MemoryLoader, reads from PageStore

AfterHistory
  +-- (unchanged)

BeforeLLM
  +-- (unchanged)

AfterToolCall
  +-- WikiExplorerHook        <- NEW: agent exploratory learning

AfterResponse
  +-- EvolutionHook           <- Refactored: writes WikiPage to PageStore
  +-- ChannelArchiveHook      <- NEW: channel message archiving

PeriodicLint (cron)
  +-- WikiLintHook            <- NEW: periodic wiki health check
```

**All hooks receive `Arc<PageStore>` + `Arc<PageIndex>` instead of `Arc<MemoryManager>`.** No hook receives a monolithic `WikiStore` — they get only the component they need.

---

## 6. Configuration

```yaml
# ~/.gasket/config.yaml - wiki section

wiki:
  enabled: true
  base_path: ~/.gasket/wiki
  sources_path: ~/.gasket/sources

  ingest:
    batch_size: 20
    auto_ingest: true
    dedup_threshold: 0.85
    max_pages_per_ingest: 15
    max_cost_per_ingest: 0.10       # USD, Deep Ingest cost cap
    cost_warning_threshold: 0.05    # Warn but proceed above this

  query:
    default_limit: 10
    hybrid_search: true
    answer_filing: true

  lint:
    enabled: true
    interval: "24h"
    auto_fix: true
    semantic_checks: true
```

---

## 7. Retrieval Strategy

For 2000+ pages, a three-tier retrieval approach:

| Tier | Technology | Purpose |
|------|-----------|---------|
| Full-text | Tantivy (BM25) | Fast keyword matching |
| Semantic | Embedding vectors | Concept-level recall |
| Metadata | SQLite FTS5 | Tag/category/time filtering |

Query flow: Tantivy retrieves top-50 candidates -> embedding reranks by semantic similarity -> budget-aware selection truncates to token limit -> full page content loaded.

---

## 8. Implementation Phases

### Phase 1: Foundation (CRUD only)
- Create `engine/src/wiki/` module with `WikiPage`, `PageStore`, `PageIndex`, `WikiLog`
- Create SQLite tables (`wiki_pages`, `raw_sources`, `wiki_relations`, `wiki_log`)
- Implement basic CRUD operations (read/write/delete/list)
- Implement disk sync (optional markdown export from SQLite)
- Update session config to use `WikiConfig`
- Replace memory tools with wiki-backed tools (keep tool names: `memorize`, `memory_search`, `memory_refresh`)
- Refactor `EvolutionHook` to use `PageStore` instead of `MemoryManager`
- Remove `engine/src/session/memory/` module
- Add `gasket wiki migrate` CLI command (one-time data migration)
- **NOT in Phase 1:** Ingest pipeline, Lint pipeline, Tantivy integration

### Phase 2: Ingest Pipeline
- Implement `SourceParser` for each format
- Implement `KnowledgeExtractor` (LLM-driven)
- Implement `WikiIntegrator` with cost validation gate
- Implement `SemanticDeduplicator`
- Add `ChannelArchiveHook` and `WikiExplorerHook`

### Phase 3: Query Pipeline
- Implement `PageIndex` with Tantivy + embedding hybrid search
- Implement `WikiQueryHook` (replaces `MemoryLoader` hook)
- Implement answer filing (good answers -> new wiki pages)
- Update agent loop to use wiki-based context loading

### Phase 4: Lint Pipeline
- Implement structural lint checks (orphans, missing pages, weak relations)
- Implement semantic lint checks (contradictions, stale claims)
- Implement `WikiLintHook` with cron scheduling
- Implement auto-fix for simple issues

### Phase 5: CLI & Polish
- Add `gasket wiki ingest <path>` command
- Add `gasket wiki query <query>` command
- Add `gasket wiki lint` command
- Add `gasket wiki list` command
- Performance optimization and testing

---

## 9. Migration from Memory to Wiki

**One-time migration command:** `gasket wiki migrate`

The old memory system is removed, but a migration CLI command provides a smooth transition:

```bash
# One-time migration: reads old memory_metadata table → creates wiki pages
gasket wiki migrate

# What it does:
# 1. Reads all rows from memory_metadata table
# 2. Creates wiki_pages rows with appropriate paths
# 3. Converts MemoryConfig settings to WikiConfig
# 4. Logs migration summary to wiki_log
```

**Removed code:**
- `engine/src/session/memory/` — entire directory
- `MemoryManager`, `MemoryWriter`, `MemoryLoader` — all types
- `MemoryConfig` in session config
- `~/.gasket/memory/` directory (replaced by `~/.gasket/wiki/`)

**Preserved code:**
- `gasket/storage/src/memory/` — kept for `EmbeddingStore`, `FileMemoryStore` patterns
- Tool names `memorize`, `memory_search`, `memory_refresh` — kept for backward compatibility

**After migration, old tables can be dropped:**
```sql
DROP TABLE IF EXISTS memory_metadata;
```

---

## 10. Tantivy Index Schema

```rust
// wiki/query/tantivy_adapter.rs

pub fn wiki_schema() -> Schema {
    let mut builder = Schema::builder();

    // Identity (path is the PK, matches wiki_pages.path)
    builder.add_text_field("path", STRING | STORED);         // "entities/projects/gasket"
    builder.add_text_field("title", TEXT | STORED);

    // Content (full-text indexed for BM25)
    builder.add_text_field("content", TEXT);                  // markdown body from SQLite
    builder.add_text_field("summary", TEXT | STORED);         // first 200 chars

    // Classification
    builder.add_text_field("type", STRING | STORED);          // entity | topic | source
    builder.add_text_field("category", STRING | STORED);
    builder.add_text_field("tags", TEXT | STORED);            // comma-separated tags

    // Temporal
    builder.add_date_field("created", STORED);
    builder.add_date_field("updated", STORED);

    // Quality
    builder.add_f64_field("confidence", STORED | INDEXED);
    builder.add_u64_field("source_count", STORED);

    builder.build()
}
```

**Document creation:** Every `PageStore::write()` call also upserts a Tantivy document via `PageIndex::upsert_document()`. Data is read from the `WikiPage` struct (which came from SQLite), not from disk files.

**Index location:** `~/.gasket/wiki/.tantivy/`

---

## 11. Concurrency Control

Since SQLite is the single truth source, concurrency control is much simpler than the original design:

- **SQLite handles write serialization** via its built-in locking (WAL mode)
- **No separate advisory lock table needed** — SQLite `INSERT OR REPLACE` is atomic
- **Disk sync is lazy** — markdown files are best-effort, regenerated from SQLite on demand

```rust
impl PageStore {
    /// Write is a single SQLite UPSERT (atomic)
    pub async fn write(&self, page: &WikiPage) -> Result<()> {
        let tags_str = serde_json::to_string(&page.tags)?;
        sqlx::query(
            r#"INSERT INTO wiki_pages (path, title, type, category, tags, content, created, updated, source_count, confidence, checksum)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(path) DO UPDATE SET
                   title = excluded.title, type = excluded.type, category = excluded.category,
                   tags = excluded.tags, content = excluded.content, updated = excluded.updated,
                   source_count = excluded.source_count, confidence = excluded.confidence,
                   checksum = excluded.checksum"#
        )
        .bind(&page.path).bind(&page.title).bind(page.page_type.as_str())
        .bind(&page.category).bind(&tags_str).bind(&page.content)
        .bind(&page.created.to_rfc3339()).bind(&page.updated.to_rfc3339())
        .bind(page.source_count).bind(page.confidence).bind(calculate_checksum(&page.content))
        .execute(&self.pool)
        .await?;

        // Lazy disk sync (fire-and-forget, best effort)
        let _ = self.sync_to_disk(page).await;
        Ok(())
    }
}
```

**No more:**
- `wiki_page_locks` table — not needed, SQLite WAL handles it
- 7-step write pipeline — reduced to 1 UPSERT + 1 optional file write
- Lock acquisition/release — SQLite does this for us

---

## 12. Single Truth Model

**SQLite is the single source of truth.** No dual-write, no sync problems.

- **Page content + metadata:** Lives in `wiki_pages` table
- **Relations:** Lives in `wiki_relations` table
- **Disk files:** Optional derived cache, regenerated from SQLite on demand

```rust
/// Sync a page from SQLite to disk (optional, for human readability)
/// Called lazily — not on every write
impl PageStore {
    pub async fn sync_to_disk(&self, page: &WikiPage) -> Result<()> {
        let path = self.wiki_root.join(format!("{}.md", page.path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, page.to_markdown()).await?;
        Ok(())
    }
}
```

**No more:**
- Link sync between frontmatter and SQLite
- `sync_links_to_disk()` on every write
- Frontmatter `outgoing_links`/`incoming_links` fields
- Cache invalidation headaches

**Relations are queried from `wiki_relations` directly.** No denormalization.

---

## 13. EvolutionHook Integration

The current `EvolutionHook` extracts structured knowledge from conversations. It is refactored to write `WikiPage` objects to `PageStore` instead of calling `MemoryManager.create_memory()`.

```rust
// hooks/evolution.rs - refactored flow

// BEFORE (current):
//   Conversation batch -> LLM extract JSON -> MemoryManager.create_memory()
//
// AFTER (wiki):
//   Conversation batch -> LLM extract JSON -> WikiPage::new() -> PageStore::write()

impl EvolutionHook {
    async fn process_batch(&self, events: &[SessionEvent], store: &PageStore) -> Result<()> {
        // 1. Extract knowledge (same LLM prompt as current, UNCHANGED)
        let items = self.llm_extract(events).await?;

        if items.is_empty() { return Ok(()); }

        // 2. Quick dedup: check if page already exists by title
        for item in &items {
            let path = format!("entities/{}/{}", slugify(&item.category), slugify(&item.title));
            if store.exists(&path).await? {
                continue; // Skip duplicate
            }

            // 3. One constructor. No special cases.
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

            // 4. Write to SQLite (single truth)
            store.write(&page).await?;
        }

        Ok(())
    }
}
```

**Two-tier ingest strategy:**
- **Quick ingest** (conversation/agent exploration): Creates 1 page per item. No LLM cost beyond initial extraction. Used by EvolutionHook.
- **Deep ingest** (file/document import): Full 10-15 page integration with cost validation gate. Higher LLM cost. Used by explicit `/ingest` command.

---

## 14. Answer Filing

Answer filing is triggered by a new agent tool `wiki_file_answer`.

```rust
// tools/wiki_file_answer.rs

pub struct WikiFileAnswerTool;

impl Tool for WikiFileAnswerTool {
    fn name(&self) -> &str { "wiki_file_answer" }
    fn description(&self) -> &str {
        "Save a valuable answer as a wiki topic page for future reference. \
         Use when the answer synthesizes multiple sources, resolves a complex \
         question, or represents knowledge worth preserving."
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let question = args["question"].as_str().ok_or(...)?;
        let answer = args["answer"].as_str().ok_or(...)?;
        let tags = args["tags"].as_array().map(|a| ...).unwrap_or_default();

        let wiki = ctx.wiki_store();
        wiki.file_answer(question, answer, tags).await?;

        Ok(ToolOutput::text("Answer filed to wiki."))
    }
}
```

The agent decides when to file based on its own judgment (the tool description guides it). No automatic trigger.

---

## 15. Tool Migration Path

**Tool names are preserved for backward compatibility.** The LLM's existing tool-use patterns (trained on `memorize`, `memory_search`) continue to work without any prompt changes.

| Tool Name | Implementation Change | Notes |
|-----------|----------------------|-------|
| `memorize` | Backed by `PageStore::write()` instead of `MemoryManager` | Same interface, writes to wiki_pages |
| `memory_search` | Backed by `PageIndex::search()` (SQLite FTS5, later Tantivy) | Same interface, better results |
| `memory_refresh` | Backed by `PageIndex::rebuild()` | Same interface, rebuilds from SQLite |
| `wiki_file_answer` (new) | Backed by `PageStore::write()` | Save good answers as wiki pages |
| `wiki_lint` (new) | Backed by `WikiLinter` | Trigger manual lint check |

**Why keep old tool names:** LLM agents develop muscle memory for tool names. Renaming `memorize` to `wiki_ingest` breaks existing prompt templates, system prompts, and agent behaviors for zero technical benefit. The implementation change is invisible to the caller.

---

## 16. Monitoring and Telemetry

Key metrics exposed via OpenTelemetry:

| Metric | Type | Purpose |
|--------|------|---------|
| `wiki.ingest.duration_ms` | Histogram | Ingest latency tracking |
| `wiki.ingest.pages_affected` | Histogram | Pages touched per ingest |
| `wiki.ingest.llm_tokens` | Counter | LLM token consumption |
| `wiki.query.duration_ms` | Histogram | Query latency |
| `wiki.query.candidates` | Histogram | Candidate pool size |
| `wiki.lint.issues_found` | Counter | Issues by category |
| `wiki.pages.total` | Gauge | Total page count |
| `wiki.relations.total` | Gauge | Total relation count |

---

## 17. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Large refactor scope (memory removal) | Phase 1 is strictly CRUD + replace; pipelines added later |
| LLM cost for Deep Ingest (10-15 pages) | **Cost validation gate** with configurable budget; falls back to Quick Ingest if over budget |
| Tantivy integration complexity | Already have `gasket/tantivy` crate in workspace; Phase 3 (after CRUD works) |
| Wiki quality degradation over time | Lint pipeline with auto-fix catches issues early |
| SQLite content storage performance | SQLite handles GB-scale text well; Tantivy provides fast search. Benchmark at 2000 pages |
| Concurrent writes | SQLite WAL mode handles serialization; no custom lock table needed |
| Migration from old memory system | One-time `gasket wiki migrate` CLI command |
| LLM rate limiting from 4 concurrent sources | Serial ingest queue per source type; configurable concurrency |
| Cost runaway | Cost validation gate + `wiki.ingest.llm_tokens` telemetry + configurable daily budget |
