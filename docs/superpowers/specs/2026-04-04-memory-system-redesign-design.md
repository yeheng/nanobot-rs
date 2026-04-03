# Memory System Redesign — Design Specification

> **Date:** 2026-04-04
> **Status:** Draft
> **Author:** Agent + User collaborative brainstorming
> **Scope:** Personal AI assistant memory system (Gasket)

---

## 1. Problem Statement

Gasket's current memory system has several gaps:

1. **Long-term memory is empty** — `~/.gasket/memory/` exists but is unused; all context relies on session-scoped event compression
2. **No human editability** — memories live in SQLite BLOBs; users cannot browse, edit, or curate their own knowledge base
3. **Single-dimension organization** — current system only has session-level compression (L0 events → L1 summaries); no multi-dimensional categorization
4. **Context explosion risk** — without structured loading strategies, adding long-term memory risks flooding the LLM context window
5. **Code-centric assumptions** — existing design inherits from code-agent paradigms (CWD, project paths); Gasket is a general-purpose personal AI assistant

## 2. Design Principles

1. **Scenario-first, not project-first** — organize by *when and how* memories are needed, not by *which codebase* they belong to
2. **Human-editable by default** — every memory is a standalone Markdown file; any text editor works
3. **Lazy loading with hard token budget** — never preload all memories; always stay within token limits
4. **Tags over structure** — topics, projects, and contexts are metadata tags, not directory hierarchies
5. **Auto-tiering with zero human effort** — frequency classification happens automatically; users may optionally refine
6. **Embedding as cross-scenario bridge** — semantic search connects memories across scenario boundaries

## 3. Architecture Overview

### 3.1 Two-Dimensional Model: Scenario × Frequency

```
                    Frequency →
Scenario    hot          warm           cold          archived
↓           (always)     (on-topic)     (on-demand)   (historical)
──────────────────────────────────────────────────────────────────
profile     [all]        —              —              —

active      [current]    [backlog]      —              —

knowledge   core         topic-match    on-search      outdated

decisions   recent       relevant       on-search      superseded

episodes    ongoing      recent         on-search      historical

reference   daily-use    topic-match    on-search      broken links
```

### 3.2 Directory Structure

```
~/.gasket/memory/
│
├── profile/                    # Scenario 1: User identity & preferences
│   ├── _INDEX.md
│   ├── preferences.md
│   ├── background.md
│   └── communication.md
│
├── active/                     # Scenario 2: Current work & focus
│   ├── _INDEX.md
│   ├── current.md
│   └── backlog.md
│
├── knowledge/                  # Scenario 3: Learned knowledge
│   ├── _INDEX.md
│   ├── rust-async-patterns.md
│   ├── cooking-tips.md
│   └── gasket-architecture.md
│
├── decisions/                  # Scenario 4: Decisions & rationale
│   ├── _INDEX.md
│   ├── chose-sqlite.md
│   ├── travel-japan-spring.md
│   └── switch-to-mac.md
│
├── episodes/                   # Scenario 5: Experiences & events
│   ├── _INDEX.md
│   ├── fixed-compactor-bug.md
│   └── tried-new-restaurant.md
│
└── reference/                  # Scenario 6: External references
    ├── _INDEX.md
    ├── useful-links.md
    └── contacts.md
```

### 3.3 Component Interaction

```
┌──────────────────────────────────────────────────────────────┐
│                     Memory System                             │
│                                                               │
│  ┌─────────────┐    ┌──────────────┐    ┌─────────────────┐  │
│  │  Filesystem  │    │   SQLite     │    │  Embedding      │  │
│  │  (.md files) │    │  (metadata + │    │  Engine         │  │
│  │              │    │   vectors)   │    │  (fastembed)    │  │
│  └──────┬───────┘    └──────┬───────┘    └────────┬────────┘  │
│         │                   │                     │            │
│         │    ┌──────────────┼─────────────────────┘            │
│         │    │              │                                  │
│         ▼    ▼              ▼                                  │
│  ┌──────────────────────────────────────┐                     │
│  │         Memory Manager               │                     │
│  │  ┌──────────┐  ┌───────────────────┐ │                     │
│  │  │  Index   │  │  Retrieval Engine │ │                     │
│  │  │  Manager │  │  (tag+embedding)  │ │                     │
│  │  └──────────┘  └───────────────────┘ │                     │
│  └──────────────────┬───────────────────┘                     │
│                     │                                         │
└─────────────────────┼─────────────────────────────────────────┘
                      │
                      ▼
              ┌───────────────┐
              │  Agent Loop   │
              │  (consumer)   │
              └───────────────┘
```

## 4. Memory File Format

### 4.1 Individual Memory File

Every memory is a standalone `.md` file with YAML frontmatter:

```markdown
---
id: mem_abc123
title: "Chose SQLite as primary storage backend"
type: decision                    # scenario-specific subtype
scenario: decisions               # which scenario bucket
tags: [gasket, database, sqlite, architecture]
frequency: warm                   # hot | warm | cold | archived
access_count: 12
created: 2026-04-01T10:00:00Z
updated: 2026-04-03T15:30:00Z
last_accessed: 2026-04-03T15:30:00Z
auto_expire: false                # true + expires date for time-limited
expires: null
tokens: 180                       # approximate token count
---

Chose SQLite over Redis for primary storage because:

- Single-user desktop application, no concurrency needs
- Zero external dependencies, bundled via rusqlite
- FTS5 provides built-in full-text search
- Event sourcing pattern maps naturally to append-only tables

Trade-off: cannot scale to multi-user, but that's not a requirement.
```

### 4.2 Scenario-Specific Subtypes

| Scenario | Valid `type` values |
|----------|-------------------|
| profile | `preference`, `background`, `communication-style`, `habit` |
| active | `current-focus`, `task`, `backlog-item`, `goal` |
| knowledge | `concept`, `convention`, `pattern`, `how-to`, `recipe` |
| decisions | `architectural`, `design`, `personal`, `planning` |
| episodes | `bug-fix`, `incident`, `milestone`, `experience`, `event` |
| reference | `link`, `contact`, `tool`, `location`, `document` |

### 4.3 Index File Format (`_INDEX.md`)

Each scenario directory contains an `_INDEX.md` that serves as both human-browsable catalog and machine-parseable manifest:

```markdown
# Knowledge Index
<!-- scenario: knowledge -->
<!-- updated: 2026-04-04T22:00:00Z -->
<!-- total_memories: 8 -->
<!-- total_tokens: ~2400 -->

## Hot (always loaded when scenario is active)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_x1 | Rust async patterns | pattern | rust,async | ~200 | Apr 4 |
| mem_x2 | Error handling | convention | rust,errors | ~150 | Apr 3 |

## Warm (loaded on topic match)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_x3 | Gasket architecture | concept | gasket,actor | ~300 | Apr 2 |

## Cold (loaded only on explicit search)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_x4 | Old deployment flow | how-to | devops | ~250 | Mar 15 |

<!-- HUMAN_NOTES_START -->
## Personal Notes
- Rust async stuff is important, always load for coding sessions
<!-- HUMAN_NOTES_END -->
```

**Key properties:**
- The section between `<!-- ... -->` header comments and `<!-- HUMAN_NOTES_START -->` is auto-generated and may be overwritten
- The `HUMAN_NOTES` section is preserved across regenerations
- Token counts are approximate, used for budget calculation
- Total tokens in header comment enables quick budget check without parsing the table

## 5. Scenario Definitions

### 5.1 Profile (`profile/`)

**Purpose:** Persistent facts about the user that shape all interactions.

**Loading:** Always loaded on session start. Never unloaded.

**Token budget:** ~200 tokens total.

**Typical contents:**
- Communication preferences (language, verbosity, tone)
- Technical background and expertise level
- Personal habits and workflow preferences
- Timezone and schedule constraints

**Frequency rules:** All profile memories are `hot`. They do not decay.

### 5.2 Active (`active/`)

**Purpose:** Track what the user is currently working on and what's pending.

**Loading:** Always loaded on session start. Updated during conversation.

**Token budget:** ~500 tokens total.

**`current.md` structure:**
```markdown
---
type: active-context
updated: 2026-04-04T22:00:00Z
---

## Current Focus
Designing Gasket's memory system, in design review phase.

## Context Notes
- Approved scenario-based approach over project-based
- User emphasized: personal AI assistant, not code agent
```

**`backlog.md` structure:**
```markdown
---
type: active-backlog
---

| Focus | Status | Last Active |
|-------|--------|-------------|
| Gasket memory system design | in-progress | Apr 4 |
| Japan trip planning | paused | Mar 28 |
| Learning Rust async | intermittent | Apr 1 |
```

**Frequency rules:**
- `current.md` is always `hot`
- When a focus area is completed or paused, its context is promoted to the appropriate scenario (knowledge/decisions/episodes)
- Backlog items that are inactive for 30+ days are moved to `cold`

### 5.3 Knowledge (`knowledge/`)

**Purpose:** Facts, patterns, conventions, and concepts the user has learned or wants to retain.

**Loading:** Loaded on topic match via tags or embedding search.

**Token budget:** ~1000 tokens per loading cycle.

**Frequency rules:**
- `hot`: Core knowledge accessed in >50% of sessions
- `warm`: Topic-relevant knowledge, accessed when matching tags detected
- `cold`: Rarely accessed, loaded only on explicit search
- Decay: warm → cold after 30 days without access; cold → archived after 90 days

### 5.4 Decisions (`decisions/`)

**Purpose:** Record choices made and their reasoning, enabling consistent future behavior.

**Loading:** Loaded when the agent detects a decision-making context or via embedding search.

**Token budget:** ~1000 tokens per loading cycle.

**Frequency rules:**
- `hot`: Decisions made in the last 7 days
- `warm`: Decisions relevant to current topic (tag match)
- `cold`: Historical decisions, loaded on search only
- Superseded decisions are marked `archived` with a `superseded_by` field pointing to the newer decision

### 5.5 Episodes (`episodes/`)

**Purpose:** Record experiences, events, and their outcomes — the "what happened" memory.

**Loading:** Primarily via embedding search. Rarely pre-loaded.

**Token budget:** Loaded on demand, counts against the on-demand budget (~1000 tokens).

**Frequency rules:**
- `hot`: Ongoing situations (unresolved issues, active investigations)
- `warm`: Recent experiences (< 14 days)
- `cold`: Historical, loaded only on semantic match
- Episodic memories naturally decay to `cold` over time

### 5.6 Reference (`reference/`)

**Purpose:** Pointers to external resources — links, contacts, tools, locations.

**Loading:** Loaded on explicit request or embedding search.

**Token budget:** Loaded on demand.

**Frequency rules:**
- `hot`: Resources used daily (e.g., CI dashboard, primary communication channel)
- `warm`: Topic-relevant resources
- `cold`: One-time-use references

## 6. Loading Strategy

### 6.1 Three-Phase Loading

```
Phase 1: Bootstrap (~700 tokens, always)
┌────────────────────────────────────────┐
│ profile/_INDEX.md + all profile/*.md   │  ~200 tokens
│ active/_INDEX.md                       │  ~50 tokens
│ active/current.md                      │  ~200 tokens
│ active/backlog.md                      │  ~250 tokens
└────────────────────────────────────────┘

Phase 2: Scenario-aware (~1500 tokens, based on agent behavior)
┌────────────────────────────────────────┐
│ Agent detects scenario:                │
│   debugging → episodes + knowledge     │
│   coding → knowledge + reference       │
│   planning → decisions + active        │
│   general → all _INDEX.md (hot items)  │
│                                        │
│ Per scenario: load _INDEX.md →         │
│   filter hot items → load files        │
└────────────────────────────────────────┘

Phase 3: On-demand (~1000 tokens, per query)
┌────────────────────────────────────────┐
│ Tag-based search in _INDEX.md          │
│ + Embedding similarity search in SQLite│
│ → Merge, deduplicate, rank             │
│ → Load top-K .md files                 │
└────────────────────────────────────────┘

Hard cap: ~3200 tokens total
```

### 6.2 Scenario Detection Heuristics

The agent infers its current behavioral scenario from conversation signals:

| Signal | Detected Scenario |
|--------|-------------------|
| Error messages, stack traces, "not working" | debugging → episodes + knowledge |
| Code snippets, implementation questions | coding → knowledge + reference |
| "should we", "which approach", "let's decide" | decision-making → decisions + knowledge |
| "what am I working on", "what's pending" | status-check → active + decisions |
| "remember this", "don't forget" | explicit-write → write to appropriate scenario |
| No clear signal | general → load all hot items |

### 6.3 Tag-Based Filtering

When a topic focus is detected (from conversation or explicit user statement), the system filters memories by tag intersection:

```
User says: "帮我看下 gasket 的 auth 相关的东西"

1. Read all _INDEX.md files
2. Filter rows where tags contain ANY of: [gasket, auth]
3. Prioritize rows where tags contain ALL of: [gasket, auth]
4. Sort by: frequency (hot > warm > cold) × tag_overlap_count
5. Load files until token budget exhausted
```

## 7. Embedding Integration

### 7.1 Data Source

Each individual `.md` memory file (frontmatter + content) is embedded as a single document.

**Not embedded:** `_INDEX.md` files, `active/current.md`, `active/backlog.md` (these are structural, always loaded).

### 7.2 Storage Schema

```sql
CREATE TABLE memory_embeddings (
    memory_path   TEXT PRIMARY KEY,       -- e.g., "knowledge/rust-async.md"
    scenario      TEXT NOT NULL,          -- profile|active|knowledge|decisions|episodes|reference
    tags          TEXT,                   -- JSON array, e.g., ["rust","async"]
    frequency     TEXT NOT NULL DEFAULT 'warm',
    embedding     BLOB NOT NULL,          -- f32 vector from fastembed
    token_count   INTEGER NOT NULL,
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_mem_emb_scenario ON memory_embeddings(scenario);
CREATE INDEX idx_mem_emb_frequency ON memory_embeddings(frequency);
```

### 7.3 Write Path

```
Memory file created/updated
    ↓
File watcher detects change
    ↓
1. Read .md file (frontmatter + content)
2. Generate embedding via local fastembed
3. UPSERT into memory_embeddings table
4. Regenerate _INDEX.md for the scenario
```

### 7.4 Query Path

```
User query
    ↓
1. Embed query text
2. SQLite cosine similarity search:
   SELECT memory_path, scenario, tags, frequency,
          cosine_similarity(embedding, ?) AS score
   FROM memory_embeddings
   WHERE frequency != 'archived'
   ORDER BY score DESC
   LIMIT 20
3. Filter by token budget
4. Load matching .md files
```

### 7.5 Embedding vs. Tag Search: When to Use Which

| Query Type | Primary Channel | Example |
|------------|----------------|---------|
| Precise topic lookup | Tag search | "gasket auth decisions" |
| Vague/fuzzy recall | Embedding search | "上次那个跟存储有关的讨论" |
| Cross-topic discovery | Embedding search | "有什么跟我现在做的事相关的" |
| Exhaustive listing | Tag search | "gasket 的所有知识" |
| Mixed (default) | Both, merged | Any general query |

**Merge algorithm:**
```
results = {}
for hit in tag_search_results:
    results[hit.path] = { source: "tag", score: tag_overlap_score }
for hit in embedding_search_results:
    if hit.path in results:
        results[hit.path].score += embedding_score * 0.5  # boost existing
    else:
        results[hit.path] = { source: "embedding", score: embedding_score }

sort by score descending
filter by token budget
```

## 8. Frequency Lifecycle

### 8.1 Auto-Decay Rules

```
hot  ── 7 days without access ──→ warm
warm ── 30 days without access ──→ cold
cold ── 90 days without access ──→ archived
```

### 8.2 Auto-Promotion Rules

```
cold ── accessed (embedding hit or tag match) ──→ warm
warm ── accessed 3+ times in 7 days ──→ hot
```

### 8.3 Frequency Update Triggers

Every time a memory file is loaded into context, the system:
1. Increments `access_count` in frontmatter
2. Updates `last_accessed` timestamp
3. Recalculates `frequency` based on decay/promotion rules
4. Updates `_INDEX.md` if frequency tier changed

### 8.4 Active Context Promotion

When a focus area in `active/current.md` is completed or paused:

```
Focus completed: "memory system design"
    ↓
Agent analyzes accumulated context:
    ├─ Conventions learned → knowledge/ (new .md file)
    ├─ Decisions made → decisions/ (new .md file)
    ├─ Experiences had → episodes/ (new .md file)
    └─ Resources used → reference/ (new .md file)
    ↓
Clear from active/current.md
Update active/backlog.md status
```

## 9. Human Editability

### 9.1 Editable Elements

Users may directly edit:
- Any memory `.md` file (frontmatter + content)
- The `HUMAN_NOTES` section in any `_INDEX.md`
- `active/current.md` and `active/backlog.md`

### 9.2 Auto-Generated Elements (overwritten on regeneration)

Users should NOT manually edit (changes will be lost):
- The table section of `_INDEX.md` (between header comments and `HUMAN_NOTES_START`)
- The `<!-- ... -->` header comments in `_INDEX.md`
- The `access_count`, `last_accessed`, `tokens` fields in frontmatter

### 9.3 File Watcher

A file watcher monitors `~/.gasket/memory/` for:
- **New files:** Generate embedding, add to SQLite, regenerate index
- **Modified files:** Re-generate embedding, update SQLite, regenerate index
- **Deleted files:** Remove from SQLite, regenerate index
- **Moved files:** Treat as delete + create

### 9.4 Conflict Resolution

If a user edits frontmatter fields that are also auto-managed:
- User edit wins for: `title`, `tags`, `frequency`, `type`
- Auto-managed wins for: `access_count`, `last_accessed`, `tokens`, `updated`
- Merge strategy: preserve user fields, update auto fields

## 10. Token Budget Enforcement

### 10.1 Budget Allocation

| Phase | Budget | Content |
|-------|--------|---------|
| Bootstrap (always) | 700 tokens | profile + active |
| Scenario-aware | 1500 tokens | scenario hot/warm items |
| On-demand | 1000 tokens | search results |
| **Total hard cap** | **3200 tokens** | |

### 10.2 Budget Enforcement Logic

```
function load_memories(query, detected_scenario):
    budget_used = 0
    loaded = []

    # Phase 1: Bootstrap (mandatory)
    for mem in bootstrap_memories:
        loaded.append(mem)
        budget_used += mem.tokens

    # Phase 2: Scenario-aware
    scenario_budget = 1500
    index = read_index(detected_scenario)
    hot_items = index.filter(frequency="hot")
    for item in hot_items:
        if budget_used + item.tokens <= TOTAL_CAP:
            loaded.append(item)
            budget_used += item.tokens
    warm_items = index.filter(frequency="warm", match_tags=query.tags)
    for item in warm_items:
        if budget_used + item.tokens <= TOTAL_CAP:
            loaded.append(item)
            budget_used += item.tokens

    # Phase 3: On-demand (embedding + tag search)
    remaining = TOTAL_CAP - budget_used
    search_results = merge_search(tag_search(query), embedding_search(query))
    for item in search_results:
        if item not in loaded and remaining >= item.tokens:
            loaded.append(item)
            remaining -= item.tokens

    return loaded
```

### 10.3 Per-File Token Tracking

Each memory file includes a `tokens` field in frontmatter. This is an approximate count (using tiktoken or similar) calculated when the file is created or modified. It enables budget checks without loading file contents.

## 11. API Surface (Rust)

### 11.1 Core Traits

```rust
/// Memory store backed by filesystem + SQLite
#[async_trait]
trait MemoryStore: Send + Sync {
    /// Create a new memory in the specified scenario
    async fn create(&self, memory: NewMemory) -> Result<MemoryId>;

    /// Read a single memory by path
    async fn read(&self, path: &str) -> Result<Memory>;

    /// Update an existing memory (rewrites the .md file)
    async fn update(&self, path: &str, content: &str) -> Result<()>;

    /// Delete a memory (removes file + SQLite entry)
    async fn delete(&self, path: &str) -> Result<()>;

    /// Search memories by tags
    async fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<MemoryHit>>;

    /// Search memories by embedding similarity
    async fn search_by_embedding(&self, query: &str, limit: usize) -> Result<Vec<MemoryHit>>;

    /// Combined search (tag + embedding, merged and ranked)
    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>>;

    /// Load memories for a given scenario, respecting token budget
    async fn load_for_scenario(
        &self,
        scenario: Scenario,
        budget: TokenBudget,
    ) -> Result<Vec<Memory>>;
}

/// Index manager for _INDEX.md files
#[async_trait]
trait IndexManager: Send + Sync {
    /// Regenerate index for a scenario (preserves human notes)
    async fn regenerate(&self, scenario: Scenario) -> Result<()>;

    /// Read index, returning parsed entries
    async fn read_index(&self, scenario: Scenario) -> Result<MemoryIndex>;
}
```

### 11.2 Key Types

```rust
enum Scenario {
    Profile,
    Active,
    Knowledge,
    Decisions,
    Episodes,
    Reference,
}

enum Frequency {
    Hot,
    Warm,
    Cold,
    Archived,
}

struct MemoryQuery {
    text: Option<String>,
    tags: Vec<String>,
    scenario: Option<Scenario>,
    max_tokens: usize,
}

struct MemoryHit {
    path: String,
    scenario: Scenario,
    title: String,
    tags: Vec<String>,
    frequency: Frequency,
    score: f32,
    tokens: usize,
}
```

## 12. Migration Path

### 12.1 From Current System

The new memory system coexists with the existing event-sourced session system:

```
Existing (unchanged):
  SessionEvent → EventStore → ContextCompactor → L1 summaries
  (handles within-session context management)

New (additive):
  Memory files → MemoryStore → MemoryManager → Agent context injection
  (handles cross-session long-term memory)
```

### 12.2 Migration Steps

1. **Phase 1 — File structure + CRUD:** Implement directory structure, file format, `MemoryStore` trait with basic create/read/update/delete
2. **Phase 2 — Index management:** Implement `_INDEX.md` generation and parsing, `IndexManager` trait
3. **Phase 3 — Loading strategy:** Implement three-phase loading with token budgets
4. **Phase 4 — Embedding integration:** Connect to existing fastembed infrastructure, implement embedding write/query paths
5. **Phase 5 — Frequency lifecycle:** Implement auto-decay, auto-promotion, and access tracking
6. **Phase 6 — File watcher:** Implement filesystem monitoring for human edits
7. **Phase 7 — Agent integration:** Wire into agent loop, add memory write triggers and retrieval hooks

### 12.3 Backward Compatibility

- Existing `~/.gasket/gasket.db` remains unchanged
- New `memory_embeddings` table is added alongside existing tables
- No changes to `EventStore`, `SessionEvent`, or `ContextCompactor`
- Memory system is opt-in: if `~/.gasket/memory/` doesn't exist, agent works as before

## 13. Open Questions

1. **File watcher implementation:** Use `notify` crate for filesystem events, or poll on access? Trade-off: complexity vs. latency.
2. **Embedding model:** Continue with existing fastembed model, or choose a different one for memory-specific embeddings?
3. **Multi-user isolation:** Current design assumes single user. If multi-user support is needed later, scope isolation strategy TBD.
4. **Memory versioning:** Should edited memories preserve history (git-like)? Or is last-write-wins sufficient?
5. **Cross-session deduplication:** When should the system detect and merge duplicate memories across sessions?
