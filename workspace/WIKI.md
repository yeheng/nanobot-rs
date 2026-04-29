---
summary: "Wiki Knowledge Management"
read_when:
  - Bootstrapping a workspace manually
---

# Wiki Knowledge Management

Gasket uses a **wiki-first knowledge system** for long-term memory. You persist important state via `wiki_write` and retrieve it via `wiki_search` or `wiki_read`.
Storage space is precious. Do not write garbage to disk.

## Wiki vs Skill — Know the Boundary

| | **Wiki** | **Skill** |
|---|---|---|
| Answers | "What" (facts, preferences, events) | "How" (procedures, workflows, SOPs) |
| Stored in | `~/.gasket/wiki/` (SQLite + Tantivy) | `workspace/skills/<name>.md` |
| Format | Markdown with YAML frontmatter | YAML frontmatter + Markdown |
| Access | Queried on-demand via tools | Loaded on demand via `read_file` |

**Rule of thumb**: If it's a fact about the user or the world → Wiki. If it's a reusable procedure with steps and pitfalls → Skill.

## Available Wiki Tools

### `wiki_search(query, limit?)`

Full-text search over the wiki knowledge base using Tantivy BM25.
- `query`: Search text (required)
- `limit`: Max results, default 10 (optional)

Returns matching pages with title, path, type, confidence score, and tags.

### `wiki_read(path)`

Read a specific wiki page by its path.
- `path`: Wiki page path, e.g. `topics/rust-async`, `entities/projects/gasket` (required)

Returns the full Markdown content and metadata (title, type, updated time, tags).

### `wiki_write(path, title, content, page_type?, tags?)`

Create or overwrite a wiki page.
- `path`: Wiki page path (required). Use directory-style naming: `topics/xxx`, `entities/xxx`, `sops/xxx`, `sources/xxx`
- `title`: Human-readable title (required)
- `content`: Markdown body (required)
- `page_type`: One of `topic` (default), `entity`, `source`, `sop`
- `tags`: Array of tags, e.g. `["rust", "async"]` (optional)

Pages are persisted to SQLite and indexed in Tantivy immediately.

## Page Types & Directory Conventions

| Type | Directory | Purpose | Example |
|------|-----------|---------|---------|
| `entity` | `entities/` | People, projects, concepts | `entities/projects/gasket` |
| `topic` | `topics/` | Concepts, guides, discussions | `topics/rust-async-patterns` |
| `source` | `sources/` | Reference materials | `sources/api-docs-openai` |
| `sop` | `sops/` | Standard operating procedures | `sops/deployment-checklist` |

## Wiki Writing Rules

- **Always search before writing** — use `wiki_search` first to avoid duplicates.
- Use meaningful paths with directory prefixes (`topics/`, `entities/`, etc.).
- Provide at least 2 descriptive `tags` for discoverability.
- Keep `title` brief and descriptive (like a commit message).
- If the user says "remember this", write to `topics/` or `entities/` by default.
- Use `sops/` for procedures with steps, pitfalls, and verification criteria.

## Storage Architecture

```
~/.gasket/wiki/            ← SQLite (SSOT) + optional .md cache
├── .tantivy/              ← Tantivy BM25 full-text search index
└── (pages stored in SQLite tables: wiki_pages, wiki_relations, wiki_log)
```

| Layer | Storage | Purpose |
|-------|---------|---------|
| Session History | SQLite (`~/.gasket/gasket.db`) | Ephemeral conversation state, auto-compacted |
| Working Memory | In-memory context window | Recent messages within token budget |
| Long-Term Knowledge | Wiki (SQLite + Tantivy) | Persistent knowledge, queried on-demand |

## Frequency Lifecycle

Wiki pages have access-frequency tiers. The system auto-adjusts priority based on access patterns:

- **Hot** (3+ accesses / 7 days) → highest priority
- **Warm** (no access 7 days) → standard priority
- **Cold** (no access 30 days) → may be cleaned up
- **Archived** (no access 90 days) → awaiting cleanup

**Exempt paths** (never decay): `profile/*`, `entities/people/*`, `sops/*`, `sources/*`, `*/decisions/*`

## Context Loading

Wiki knowledge is **not automatically injected** into the system prompt. The agent must proactively call `wiki_search` to retrieve relevant knowledge before responding. This is intentional — it keeps the system prompt stable for prompt caching and reduces token waste.

**Preparation Protocol**: Before responding to any user query, always use `wiki_search` to check if relevant knowledge already exists in the wiki. 
