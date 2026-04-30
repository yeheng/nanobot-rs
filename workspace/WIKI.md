---
summary: "Wiki knowledge system overview and principles"
---

# Wiki Knowledge System

Wiki-first long-term memory. Persist via `wiki_write`, retrieve via `wiki_search` / `wiki_read`.

## Wiki vs Skill

| | Wiki | Skill |
|---|---|---|
| Answers | "What" (facts, preferences) | "How" (procedures, SOPs) |
| Stored | `~/.gasket/wiki/` (SQLite + Tantivy) | `workspace/skills/<name>/SKILL.md` |
| Format | Markdown + YAML frontmatter | Markdown + YAML frontmatter |

Rule of thumb: fact → Wiki; reusable procedure → Skill.

## Storage Layers

| Layer | Storage | Purpose |
|-------|---------|---------|
| Session | SQLite (`gasket.db`) | Ephemeral conversation state |
| Working | In-memory context | Recent messages |
| Long-term | Wiki (SQLite + Tantivy) | Persistent knowledge |

## Principles

- **Search before write** — avoid duplicates.
- **Not auto-injected** — call `wiki_search` proactively.
- **Path conventions**: `topics/`, `entities/`, `sources/`, `sops/`.
- **Frequency tiers**: Hot (3+/7d) → Warm (7d) → Cold (30d) → Archived (90d). Exempt: `profile/*`, `entities/people/*`, `sops/*`, `sources/*`, `*/decisions/*`.

For tool signatures and operations, see **wiki skill** (`workspace/skills/wiki/SKILL.md`).
