---
name: memory
description: Manage long-term memory using gasket's hybrid file + SQLite system
always: false
---

# Memory Management Skill

This skill provides guidance on using gasket's long-term memory system with file-backed storage and SQLite indexing.

## Overview

Gasket uses a **hybrid memory system** combining Markdown files (human-readable) with SQLite (fast retrieval):

### Storage Locations

1. **Bootstrap Files** (loaded once at startup from workspace):
   - `workspace/MEMORY.md` — Human-written guidelines (~2048 token hard limit)
   - `workspace/PROFILE.md`, `SOUL.md`, `AGENTS.md` — Identity files
   - `workspace/skills/*/SKILL.md` — Skill definitions

2. **Long-term Memory** (persistent, at `~/.gasket/memory/`):
   ```
   ~/.gasket/memory/
   ├── profile/           # User preferences (exempt from decay)
   ├── active/            # Current projects (subject to decay)
   ├── knowledge/         # Learned facts (subject to decay)  
   ├── decisions/         # Architecture decisions (exempt from decay)
   ├── episodes/          # Past events (subject to decay)
   ├── reference/         # External resources (exempt from decay)
   └── .history/          # Versioned backups of edits
   ```

3. **SQLite Index** (at `~/.gasket/gasket.db`):
   - `memory_metadata` table — indexed metadata for fast queries
   - `memory_embeddings` table — vector embeddings for semantic search

### File Format

Memory files use UUID naming with YAML frontmatter:

```markdown
---
id: mem_019d6784-60c8-7a33-b10a-19069d1d9f5b
title: 用户偏好：城市与时区
type: note
scenario: profile
tags: [user, timezone, guangzhou, preference]
frequency: warm
access_count: 0
created: 2026-04-07T10:37:02.025032+00:00
updated: 2026-04-07T10:37:02.025032+00:00
tokens: 19
---

## Content here
```

## When to Use Memory

Use the memory system to store and retrieve:
- **Important user preferences** (e.g., timezone, language preferences)
- **Key project facts** (e.g., "Project uses PostgreSQL database")
- **Recurring patterns or decisions** (e.g., "Always use async/await for I/O operations")
- **Architecture decisions** and their rationale
- **Daily notes** and conversation summaries

## How to Write to Memory

### Using the MemorizeTool (Recommended)

Agents use the `memorize` tool to write memories:

```rust
memorize(
    title: "用户偏好：城市与时区",
    content: "用户位于广州，使用 GMT+8 时区...",
    scenario: "profile",  // profile | active | knowledge | decisions | episodes | reference
    tags: ["user", "timezone", "preference"]
)
```

This automatically:
- Generates a UUID for the memory
- Adds YAML frontmatter with metadata
- Writes to `~/.gasket/memory/<scenario>/mem_<UUID>.md`
- Updates SQLite metadata index
- Creates backup in `.history/`

### Manual File Operations

You can also manually edit files in `~/.gasket/memory/`:

```bash
# Reindex after manual edits
gasket memory reindex
```

**Always read before writing** — use `memory_search` or `read_file` first to avoid duplicates.

## How to Read Memory

### Three-Phase Loading (Automatic)

Memory is loaded automatically based on context:

1. **Phase 1 (Bootstrap, ~700 tokens):** Always loads profile + active memories
2. **Phase 2 (Scenario, ~1500 tokens):** Scenario-specific hot/warm items
3. **Phase 3 (On-demand, ~1000 tokens):** Fills remaining budget via semantic search

Total never exceeds ~3200 tokens default budget.

### Using MemorySearchTool

Agents use the `memory_search` tool:

```rust
memory_search(
    query: "timezone preferences",
    tags: ["user", "preference"],
    limit: 10
)
```

This queries the SQLite metadata store for fast retrieval, with fallback to filesystem scan.

### Manual Reading

```bash
# Browse memory files directly
ls ~/.gasket/memory/profile/
cat ~/.gasket/memory/knowledge/mem_*.md

# Search with grep
grep -r "timezone" ~/.gasket/memory/
```

## Best Practices

1. **Use Appropriate Scenarios**:
   - `profile/` — User preferences (persistent, never decays)
   - `active/` — Current project context (decays when inactive)
   - `knowledge/` — General facts (decays if unused)
   - `decisions/` — Architecture decisions (persistent)
   - `episodes/` — Past events/conversations (decays)
   - `reference/` — External resources (persistent)

2. **Write Descriptive Titles**: Titles are indexed and used for quick scanning.

3. **Use Tags Liberally**: Tags enable fast filtering (`user`, `project-alpha`, `preference`, etc.).

4. **Track Frequency**: Set `frequency: hot|warm|cold` to control retention priority.

5. **Keep Atomic**: One concept per memory file for easier retrieval.

## Memory Lifecycle

### Decay System

Memories have a lifecycle based on usage:

- **Hot** (`frequency: hot`) — Recently accessed, never decays
- **Warm** (`frequency: warm`) — Used occasionally, slow decay
- **Cold** (`frequency: cold`) — Unused, eligible for pruning

Access count is tracked automatically. Use `gasket memory reindex` to refresh metadata.

### Token Budgets

Memory loading respects token limits:
- `MEMORY.md` in workspace: ~2048 tokens (hard truncation)
- Runtime context injection: ~3200 tokens total (three-phase loading)

## Important Notes

- **Persistence**: Memory at `~/.gasket/memory/` persists across sessions and system restarts.
- **Human-Readable**: All memories are Markdown files (git-friendly, editable with any text editor).
- **SQLite Index**: Metadata and embeddings are cached in `~/.gasket/gasket.db` for fast queries.
- **Graceful Degradation**: System works even if `~/.gasket/memory/` doesn't exist (skips memory loading).
- **History Tracking**: All edits are backed up to `.history/` for versioning.

## CLI Commands

```bash
# Rebuild SQLite index from filesystem
gasket memory reindex

# View memory statistics
gasket memory stats
```

## Integration with Agent Loop

- **At startup**: `AgentLoop::build_internal()` initializes `MemoryManager` if `~/.gasket/memory/` exists
- **Per request**: `AgentLoop::load_memory_context()` queries memories based on user message
- **Injection**: Results formatted as "## Memories" section in prompt escalation
