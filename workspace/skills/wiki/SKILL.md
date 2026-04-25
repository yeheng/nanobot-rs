---
name: wiki
description: Operational guide for gasket's wiki knowledge system
always: false
---

# Wiki Skill

Operational guide for reading, writing, and managing wiki knowledge pages.

## Path Conventions

| Content Type | Path Pattern | page_type |
|---|---|---|
| General knowledge | `topics/<slug>` | `topic` |
| People, projects, teams | `entities/<slug>` | `entity` |
| External references, URLs | `sources/<slug>` | `source` |
| Step-by-step procedures | `sop/<slug>` | `sop` |

## Common Operations

### Write a Page

```
wiki_write(
    path: "topics/rust-async-patterns",
    title: "Rust Async Patterns",
    content: "## Overview\n...",
    page_type: "topic",
    tags: ["rust", "async"]
)
```

### Search Pages

```
wiki_search(query: "database design", limit: 10)
```

### Read a Page

```
wiki_read(path: "topics/rust-async-patterns")
```

### Refresh Index

```
wiki_refresh(action: "sync")     # Sync changed files only
wiki_refresh(action: "reindex")  # Full rebuild
wiki_refresh(action: "stats")    # Show statistics
```

## Best Practices

1. **Search before write** — use `wiki_search` first to avoid duplicates.
2. **One concept per page** — easier retrieval and lifecycle management.
3. **Use descriptive paths** — `topics/event-sourcing-design` over `topics/note-1`.
4. **At least 2 tags** — tags improve search quality.
5. **Use entities for people/projects** — `entities/people/alice`, `entities/projects/gasket`.

## Manual Operations

```bash
# Browse wiki files
ls ~/.gasket/wiki/topics/
cat ~/.gasket/wiki/topics/rust-async-patterns.md

# Search via CLI
gasket wiki search <query>

# Rebuild index after manual edits
gasket wiki reindex
```
