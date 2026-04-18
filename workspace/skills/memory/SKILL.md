---
name: memory
description: Operational guide for gasket's long-term memory system
always: false
---

# Memory Skill

Operational guide for reading, writing, and managing long-term memories.
For the memory ABI (scenarios, writing rules, frequency lifecycle), see `workspace/MEMORY.md`.

## Choosing the Right Scenario

| User Says | Scenario | Example |
|-----------|----------|---------|
| "remember my preference" | `profile` | timezone, language, coding style |
| "I'm working on X" | `active` | current sprint, ongoing refactor |
| "I learned that..." | `knowledge` | a framework quirk, a useful pattern |
| "we decided to..." | `decisions` | chose PostgreSQL over MongoDB because... |
| "yesterday we fixed..." | `episodes` | debugged auth timeout, root cause was... |
| "bookmark this link" | `reference` | API docs URL, Grafana dashboard |

## Common Operations

### Write a Memory

```rust
memorize(
    title: "User codes in Rust",
    content: "User prefers Rust for backend, TypeScript for frontend...",
    scenario: "profile",
    tags: ["user", "language", "preference"]
)
```

### Search Memories

```rust
// By tags
memory_search(tags: ["user", "preference"], limit: 5)

// By text query
memory_search(query: "timezone preferences", limit: 10)

// By scenario + tags
memory_search(query: "database choice", tags: ["architecture"], limit: 5)
```

### Update a Memory

```rust
update_memory(
    scenario: "active",
    filename: "mem_019d6784.md",
    content: "Updated project status: Phase 2 complete..."
)
```

### Delete a Memory

```rust
delete_memory(
    scenario: "active",
    filename: "mem_019d6784.md"
)
```

## Examples

### Store a decision record

```rust
memorize(
    title: "ADR-001: Use event sourcing for sessions",
    content: "## Context\nSession history needs replay capability.\n\n## Decision\nEventStore with linear sequence numbers.\n\n## Consequences\n- Enables session restoration\n- Requires periodic compaction",
    scenario: "decisions",
    tags: ["adr", "event-sourcing", "session"]
)
```

### Store a debugging episode

```rust
memorize(
    title: "Fixed: CronService compilation error",
    content: "## Problem\nMissing `clone_box` method on TextEmbedder trait.\n## Root Cause\nTrait object needs boxed clone support.\n## Fix\nAdded `fn clone_box(&self) -> Box<dyn TextEmbedder>`.",
    scenario: "episodes",
    tags: ["debug", "trait-objects", "rust"]
)
```

### Store a reference

```rust
memorize(
    title: "Grafana: API Latency Dashboard",
    content: "URL: grafana.internal/d/api-latency\nUsed by oncall team. Alert threshold: p99 > 500ms.",
    scenario: "reference",
    tags: ["monitoring", "grafana", "api-latency"]
)
```

## Manual Operations

```bash
# Browse memory files
ls ~/.gasket/memory/profile/
cat ~/.gasket/memory/knowledge/mem_*.md

# Search across all memories
grep -r "keyword" ~/.gasket/memory/

# Rebuild SQLite cache after manual edits
gasket memory reindex

# View statistics
gasket memory stats
```

After manual file edits, always run `gasket memory reindex` to sync the SQLite cache.

## Best Practices

1. **Read before write** — use `memory_search` first to avoid duplicates.
2. **One concept per memory** — easier retrieval and lifecycle management.
3. **At least 2 tags** — tags drive filtering in Phase 2 scenario loading.
4. **Descriptive titles** — indexed and shown in search results.
5. **Use exempt scenarios freely** — `profile`, `decisions`, `reference` never decay.
6. **Don't rely on filenames** — the system sorts by frequency, not name patterns.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Memory not showing in context | Check frequency tier; cold memories only load via search |
| Duplicate memories found | Always `memory_search` before `memorize` |
| SQLite out of sync | Run `gasket memory reindex` |
| Embeddings missing | Reindex will recompute; or check embedder config |
| File corrupted | Skipped automatically; check `.history/` for backup |
