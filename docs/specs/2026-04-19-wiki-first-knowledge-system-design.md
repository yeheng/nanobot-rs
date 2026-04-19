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

Every wiki page uses Markdown with YAML frontmatter:

```markdown
---
id: wiki://entities/projects/gasket
title: "Gasket Project"
type: entity
category: project
tags: [rust, agent, llm, mcp]
created: 2026-04-19T10:00:00Z
updated: 2026-04-19T15:30:00Z
source_count: 12
confidence: 0.95
outgoing_links:
  - wiki://entities/concepts/actor-model
  - wiki://entities/people/yeheng
incoming_links:
  - wiki://topics/architecture-overview
---

# Gasket Project

## Overview
Gasket is a Rust LLM Agent framework...

## Key Decisions
- 2026-03: Chose Actor Model as core architecture...

## Related Entities
- [[actor-model]] - Core architecture pattern
- [[yeheng]] - Project lead

## Sources
- [karpathy-llm-wiki](source://2026-04-19-karpathy-llm-wiki)
```

### 3.2 URI Scheme

```
wiki://entities/projects/gasket    -> Wiki page
wiki://entities/people/yeheng      -> Wiki page
wiki://topics/architecture-overview -> Wiki page
wiki://sources/2026-04-19-xxx      -> Source summary page
wiki://index                       -> Index page
wiki://log                         -> Log page
source://uploads/2026-04-19-paper  -> Raw source reference
```

URI resolution maps to filesystem paths under `~/.gasket/wiki/` or `~/.gasket/sources/`.

### 3.3 SQLite Schema

```sql
-- Wiki page metadata
CREATE TABLE wiki_pages (
    id          TEXT PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    title       TEXT NOT NULL,
    type        TEXT NOT NULL,           -- entity | topic | source
    category    TEXT,
    tags        TEXT,                    -- JSON array
    created     TEXT NOT NULL,
    updated     TEXT NOT NULL,
    source_count INTEGER DEFAULT 0,
    confidence  REAL DEFAULT 1.0,
    checksum    TEXT
);

-- Raw source registry
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
    from_page   TEXT NOT NULL,
    to_page     TEXT NOT NULL,
    relation    TEXT NOT NULL,           -- references | contradicts | supersedes | related
    confidence  REAL DEFAULT 1.0,
    created     TEXT NOT NULL,
    PRIMARY KEY (from_page, to_page, relation)
);

-- Structured operation log (mirrors log.md)
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

---

## 4. Core Operations

### 4.1 Ingest Pipeline

Ingest converts raw information into structured wiki pages.

**Two-tier strategy:**
- **Quick Ingest** (1 page per item): Used for conversation extraction and agent exploration. Low LLM cost. Creates one page + updates index.
- **Deep Ingest** (up to 15 pages per source): Used for explicit file/document import. Higher LLM cost. Full integration with entity/topic cross-referencing.

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

Query replaces the existing MemoryLoader with unified wiki retrieval.

```rust
pub struct WikiQueryEngine {
    tantivy: TantivySearch,
    fts5: SqliteFts,
    embedding: EmbeddingStore,
    page_resolver: PageResolver,
}

impl WikiQueryEngine {
    pub async fn query(&self, query: &str, budget: TokenBudget) -> Result<QueryResult> {
        // Phase 1: Candidate retrieval (Tantivy hybrid search)
        let candidates = self.tantivy.search(query, 50).await?;

        // Phase 2: Semantic reranking (embedding similarity)
        let reranked = self.embedding.rerank(query, &candidates).await?;

        // Phase 3: Budget-aware selection
        let selected = self.apply_budget(reranked, budget);

        // Load full page content
        let pages = self.load_pages(&selected).await?;

        Ok(QueryResult { pages, total_candidates: candidates.len() })
    }

    /// File a good answer back into the wiki as a new topic page
    pub async fn file_answer(&self, question: &str, answer: &str, wiki: &WikiStore) -> Result<()> {
        let page = WikiPage::new_topic(question, answer);
        wiki.write_page(&page.id, &page).await?;
        wiki.rebuild_index().await?;
        Ok(())
    }
}
```

### 4.3 Lint Pipeline

Lint is a new capability for wiki health checking.

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
    pub async fn lint(&self, wiki: &WikiStore) -> Result<LintReport> {
        let all_pages = wiki.list_all_pages().await?;

        // Structural checks (fast, no LLM)
        let orphans = Self::find_orphans(&all_pages)?;
        let missing_pages = Self::find_missing_pages(&all_pages)?;
        let weak_relations = Self::find_weak_relations(&all_pages)?;

        // Semantic checks (slow, LLM-driven)
        let contradictions = self.llm_find_contradictions(&all_pages).await?;
        let stale_claims = self.llm_find_stale_claims(&all_pages).await?;

        let report = LintReport { contradictions, stale_claims, orphans, missing_pages, weak_relations };

        wiki.append_log(&format!(
            "## [{}] lint | {} issues found\n",
            chrono::now().format("%Y-%m-%d"),
            report.total_issues(),
        )).await?;

        Ok(report)
    }

    pub async fn auto_fix(&self, report: &LintReport, wiki: &WikiStore) -> Result<FixReport> {
        // Auto-fix: link orphans, create missing page stubs
        // ...
    }
}
```

---

## 5. Module Architecture

### 5.1 Rust Module Structure

```
gasket/engine/src/
+-- wiki/                          # Wiki module (replaces memory/)
|   +-- mod.rs                     # WikiStore facade
|   +-- page.rs                    # WikiPage data model
|   +-- uri.rs                     # wiki:// URI scheme
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
|   +-- index.rs                   # index.md maintenance
|   +-- log.rs                     # log.md maintenance
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

### 5.2 Key Interfaces

```rust
// wiki/mod.rs - Core facade

pub struct WikiStore {
    base_path: PathBuf,
    db: SqliteStore,
    tantivy: TantivyIndex,
}

impl WikiStore {
    // CRUD
    pub async fn read_page(&self, id: &WikiUri) -> Result<WikiPage>;
    pub async fn write_page(&self, id: &WikiUri, page: &WikiPage) -> Result<()>;
    pub async fn delete_page(&self, id: &WikiUri) -> Result<()>;
    pub async fn list_pages(&self, filter: PageFilter) -> Result<Vec<PageSummary>>;

    // Relations
    pub async fn add_relation(&self, from: &WikiUri, to: &WikiUri, rel: Relation) -> Result<()>;
    pub async fn get_relations(&self, id: &WikiUri) -> Result<Vec<WikiRelation>>;

    // Sources
    pub async fn register_source(&self, source: RawSource) -> Result<SourceId>;
    pub async fn mark_ingested(&self, id: &SourceId) -> Result<()>;

    // Index & Log
    pub async fn rebuild_index(&self) -> Result<()>;
    pub async fn append_log(&self, entry: &str) -> Result<()>;

    // Search
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<PageSummary>>;
}

// wiki/uri.rs

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WikiUri {
    Page { path: String },
    Source { id: String },
    Index,
    Log,
}

// wiki/page.rs

#[derive(Debug, Clone)]
pub struct WikiPage {
    pub id: WikiUri,
    pub frontmatter: PageFrontmatter,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageFrontmatter {
    pub title: String,
    #[serde(rename = "type")]
    pub page_type: PageType,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub source_count: Option<u32>,
    pub confidence: Option<f64>,
    pub outgoing_links: Vec<WikiUri>,
    pub incoming_links: Vec<WikiUri>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PageType { Entity, Topic, Source }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Relation { References, Contradicts, Supersedes, Related }
```

### 5.3 Hook Integration

```
Hook Pipeline (extended):

BeforeRequest
  +-- WikiQueryHook           <- Replaces MemoryLoader

AfterHistory
  +-- (unchanged)

BeforeLLM
  +-- (unchanged)

AfterToolCall
  +-- WikiExplorerHook        <- NEW: agent exploratory learning

AfterResponse
  +-- EvolutionHook           <- Refactored: Quick Ingest (1 page per item)
  +-- ChannelArchiveHook      <- NEW: channel message archiving -> Quick Ingest

PeriodicLint (cron)
  +-- WikiLintHook            <- NEW: periodic wiki health check
```

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

### Phase 1: Foundation
- Create `engine/src/wiki/` module with `WikiStore`, `WikiPage`, `WikiUri`
- Create SQLite tables (`wiki_pages`, `raw_sources`, `wiki_relations`, `wiki_log`)
- Implement basic CRUD operations
- Remove `engine/src/session/memory/` module
- Update all `MemoryManager` references to `WikiStore`

### Phase 2: Ingest Pipeline
- Implement `SourceParser` for each format
- Implement `KnowledgeExtractor` (LLM-driven)
- Implement `WikiIntegrator` (affects 10-15 pages per source)
- Implement `SemanticDeduplicator`
- Refactor `EvolutionHook` to use Wiki Ingest
- Add `ChannelArchiveHook` and `WikiExplorerHook`
- Implement `index.md` and `log.md` auto-maintenance

### Phase 3: Query Pipeline
- Implement `WikiQueryEngine` with Tantivy + embedding hybrid search
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

## 9. What Gets Removed

The following are cleanly removed (no backward compatibility, no migration):

- `engine/src/session/memory/` - entire directory
- `MemoryManager`, `MemoryWriter`, `MemoryLoader` - all types
- `MemoryConfig` in session config
- `memory_metadata` SQLite table (replaced by `wiki_pages`)
- `~/.gasket/memory/` directory (replaced by `~/.gasket/wiki/`)
- All `MemoryManager` references in agent loop, hooks, and session code

**Design decision:** No migration path from old memory files. This is a clean architectural break. Users start fresh with the wiki system.

---

## 10. Tantivy Index Schema

```rust
// wiki/query/tantivy_adapter.rs

pub fn wiki_schema() -> Schema {
    let mut builder = Schema::builder();

    // Identity fields
    builder.add_text_field("id", STRING | STORED);           // wiki:// URI
    builder.add_text_field("path", STRING | STORED);         // filesystem path
    builder.add_text_field("title", TEXT | STORED);          // page title

    // Content (full-text indexed for BM25)
    builder.add_text_field("content", TEXT);                  // markdown body
    builder.add_text_field("summary", TEXT | STORED);         // first 200 chars

    // Classification
    builder.add_text_field("type", STRING | STORED);          // entity | topic | source
    builder.add_text_field("category", STRING | STORED);      // subcategory
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

**Document creation:** Every `write_page()` call also upserts a Tantivy document. The `content` field receives the full markdown body (without frontmatter). The `summary` field receives the first 200 characters for snippet display.

---

## 11. Concurrency Control

Wiki pages are updated concurrently from multiple ingest sources. A file-level advisory locking mechanism prevents lost updates.

```rust
// wiki/mod.rs - concurrency control

impl WikiStore {
    /// Write a page with advisory locking
    pub async fn write_page(&self, id: &WikiUri, page: &WikiPage) -> Result<()> {
        let path = id.to_file_path(&self.base_path);

        // 1. Acquire file-level lock (SQLite-backed, async-safe)
        let lock = self.db.acquire_page_lock(id.as_str()).await?;

        // 2. Read current version
        let current = self.read_page_raw(&path).await.ok();

        // 3. Merge frontmatter links (SQLite is truth for links)
        let merged = self.merge_link_metadata(page, current.as_ref()).await?;

        // 4. Write to filesystem
        self.write_page_raw(&path, &merged).await?;

        // 5. Update SQLite metadata
        self.db.upsert_page_metadata(&merged.frontmatter).await?;

        // 6. Update Tantivy index
        self.tantivy.upsert_document(id, &merged).await?;

        // 7. Release lock
        drop(lock);

        Ok(())
    }
}
```

**Lock implementation:** Uses a `wiki_page_locks` SQLite table with `page_id` as PK and `locked_at` timestamp. Locks auto-expire after 30 seconds to handle crashes.

---

## 12. Link Truth Model

**SQLite is the source of truth for links.** Frontmatter `outgoing_links` and `incoming_links` are derived fields, synchronized from SQLite.

```rust
/// Sync links from SQLite → filesystem frontmatter
/// Called after every batch of relation writes and during Lint
pub async fn sync_links_to_disk(&self, page_id: &WikiUri) -> Result<()> {
    let outgoing = self.db.get_outgoing_relations(page_id).await?;
    let incoming = self.db.get_incoming_relations(page_id).await?;

    let mut page = self.read_page(page_id).await?;
    page.frontmatter.outgoing_links = outgoing;
    page.frontmatter.incoming_links = incoming;
    page.frontmatter.updated = Utc::now();

    self.write_page_raw(&page_id.to_file_path(&self.base_path), &page).await?;
    Ok(())
}
```

**Lifecycle:** Ingest writes relations to SQLite -> `sync_links_to_disk()` updates frontmatter for affected pages -> Lint verifies consistency.

---

## 13. EvolutionHook Integration

The current `EvolutionHook` extracts structured knowledge from conversations. It is refactored to output `IngestRequest` objects instead of calling `MemoryManager.create_memory()` directly.

```rust
// hooks/evolution.rs - refactored flow

// BEFORE (current):
//   Conversation batch -> LLM extract JSON -> MemoryManager.create_memory()
//
// AFTER (wiki):
//   Conversation batch -> LLM extract JSON -> IngestRequest -> WikiIntegrator

pub struct IngestRequest {
    pub title: String,
    pub content: String,
    pub source_type: SourceFormat::Conversation,
    pub tags: Vec<String>,
    pub category: Option<String>,
    pub entities: Vec<String>,      // extracted entity names
    pub relations: Vec<String>,     // related topic names
}

impl EvolutionHook {
    async fn process_batch(&self, events: &[SessionEvent], wiki: &WikiStore) -> Result<()> {
        // 1. Extract knowledge (same LLM prompt as current, unchanged)
        let items = self.llm_extract(events).await?;

        if items.is_empty() { return Ok(()); }

        // 2. Deduplicate against existing wiki pages
        let deduped = self.dedup.filter(items, wiki).await?;

        // 3. Convert to IngestRequest (lightweight, no 10-15 page expansion)
        for item in deduped {
            let request = IngestRequest::from_evolution_item(&item);

            // Quick ingest: creates 1 entity/topic page + updates index
            // Does NOT trigger full 10-15 page expansion
            wiki.quick_ingest(request).await?;
        }

        Ok(())
    }
}
```

**Two-tier ingest strategy:**
- **Quick ingest** (conversation/agent exploration): Creates 1 page per item, updates index. Low LLM cost. Used by EvolutionHook.
- **Deep ingest** (file/document import): Full 10-15 page integration. Higher LLM cost. Used by explicit `/ingest` command.

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

| Old Tool | New Tool | Notes |
|----------|----------|-------|
| `memory_search` | `wiki_search` | Same interface, backed by Tantivy |
| `memory_refresh` | `wiki_refresh` | Rebuilds index from filesystem |
| `memorize` | `wiki_ingest` | Enhanced: creates page + updates related pages |
| (new) | `wiki_file_answer` | Save good answers as wiki pages |
| (new) | `wiki_lint` | Trigger manual lint check |

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
| LLM cost for Deep Ingest (10-15 pages) | Two-tier ingest: Quick (1 page, cheap) vs Deep (15 pages, explicit only) |
| Tantivy integration complexity | Already have `gasket/tantivy` crate in workspace |
| Wiki quality degradation over time | Lint pipeline with auto-fix catches issues early |
| URI resolution edge cases | Comprehensive test suite for `WikiUri` parser |
| Concurrent writes causing lost updates | SQLite-backed advisory locking with auto-expiry |
| Frontmatter/SQLite link inconsistency | SQLite is truth; `sync_links_to_disk()` on write and lint |
| LLM rate limiting from 4 concurrent sources | Serial ingest queue per source type; configurable concurrency |
| Cost runaway | `wiki.ingest.llm_tokens` telemetry + configurable daily budget |
