---
name: wiki
description: Read, write, and manage wiki knowledge pages
always: false
---

# Wiki Operations

## Path Conventions

| Content | Path | page_type |
|---------|------|-----------|
| General knowledge | `topics/<slug>` | `topic` |
| People, projects | `entities/<slug>` | `entity` |
| References | `sources/<slug>` | `source` |
| Procedures | `sops/<slug>` | `sop` |

## Rules

1. **Search before write** — avoid duplicates.
2. **One concept per page** — easier retrieval.
3. **≥2 tags** — improves search.
4. **Entities for people/projects** — `entities/people/alice`, `entities/projects/gasket`.
