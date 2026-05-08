---
summary: "Wiki knowledge system overview and principles"
read_when:
  - Every conversation start
---

# Wiki Knowledge System

Wiki-first long-term memory. Persist via `wiki_write`, retrieve via `wiki_search` / `wiki_read`.

## Wiki vs Skill

| | Wiki | Skill |
|---|---|---|
| Answers | "What" (facts, preferences) | "How" (procedures, SOPs) |
| Stored | `~/.gasket/wiki/` | `workspace/skills/<name>/SKILL.md` |
| Format | Markdown + YAML frontmatter | Markdown + YAML frontmatter |

Rule of thumb: fact → Wiki; reusable procedure → Skill.

## Principles

- **Search before write** — avoid duplicates.
- **Not auto-injected** — call `wiki_search` proactively.
- **Path conventions**: `topics/`, `entities/`, `sources/`, `sops/`. Pages MUST be under one of these prefixes. Root-level wiki pages are forbidden.

