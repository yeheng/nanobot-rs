---
name: memory
description: Manage long-term memory using markdown files
always: false
---

# Memory Management Skill

This skill provides guidance on using nanobot's file-based long-term memory system.

## Overview

All long-term memory lives in **Markdown files** — the single source of truth:

- **`MEMORY.md`** — Core facts, user preferences, and pointers to detailed files. Loaded into every conversation automatically (keep under ~2000 tokens).
- **`memory/*.md`** — Detailed project context, daily notes, logs. Unlimited size.

There is no database for explicit memories. Files are the only storage medium.

## When to Use Memory

Use the memory system to store and retrieve:
- Important user preferences (e.g., "User prefers Python over JavaScript")
- Key facts about projects (e.g., "Project uses PostgreSQL database")
- Recurring patterns or decisions (e.g., "Always use async/await for I/O operations")
- Daily notes and conversation summaries

## How to Write to Memory

Use `write_file` or `edit_file` to update files in the memory directory:

```
# Example: Writing to MEMORY.md
edit_file: ~/.nanobot/memory/MEMORY.md

# Example: Creating a project-specific file
write_file: ~/.nanobot/memory/project_alpha.md
```

**Always read before writing** — use `read_file` first to avoid overwriting existing content.

## How to Read Memory

1. **`MEMORY.md`** — already in your context, check it first.
2. **`read_file`** — use when you know which file has the answer.
3. **`memory_search`** — keyword search across all `memory/*.md` files when you don't know where something is.

## Best Practices

1. **Be Selective**: Only store truly important, reusable, and enduring information in `MEMORY.md`.
2. **Be Concise**: Keep entries brief and searchable.
3. **Use Categories**: Organize `MEMORY.md` with clear sections.
4. **Offload Details**: Put detailed context in `memory/*.md` files, leave short pointers in `MEMORY.md`.
5. **Review Regularly**: Periodically review and clean up `MEMORY.md` to keep it under the token limit.

## Memory Window

The agent has a limited memory window in a single session (e.g., 50 messages).
- Long conversations will have older messages summarized automatically.
- Important facts learned during the session should be explicitly saved to memory files.

## Important Notes

- Memory files persist across system restarts and sessions.
- All knowledge is stored in plain Markdown files (human-readable, git-friendly, editable).
- Use memory intelligently to avoid asking repetitive questions and to maintain context.
