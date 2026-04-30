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

## Tools

```
wiki_search(query, limit=10)           # full-text search
wiki_read(path)                        # read page
wiki_write(path, title, content, page_type="topic", tags=[])  # write page
wiki_refresh(action: "sync|reindex|stats")  # index management
```

## Rules

1. **Search before write** — avoid duplicates.
2. **One concept per page** — easier retrieval.
3. **≥2 tags** — improves search.
4. **Entities for people/projects** — `entities/people/alice`, `entities/projects/gasket`.

## CLI

```bash
gasket wiki search <query>
gasket wiki reindex
ls ~/.gasket/wiki/topics/
```
