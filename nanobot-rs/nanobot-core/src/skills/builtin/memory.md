---
name: memory
description: Manage long-term memory using MEMORY.md and HISTORY.md
always: false
---

# Memory Management Skill

This skill provides guidance on using nanobot's long-term memory system.

## Overview

nanobot has a two-tier memory system:

1. **MEMORY.md** - Long-term memory for important facts, preferences, and knowledge
2. **HISTORY.md** - Chronological log of events and activities

## When to Use Memory

Use memory to store:
- Important user preferences (e.g., "User prefers Python over JavaScript")
- Key facts about projects (e.g., "Project uses PostgreSQL database")
- Recurring patterns or decisions (e.g., "Always use async/await for I/O operations")
- User information (e.g., "User's timezone is UTC+8")

## How to Write to Memory

### MEMORY.md

Use `write_file` or `edit_file` to add information:

```
Use write_file to create:
~/.nanobot/memory/MEMORY.md

Content format:
# Long-Term Memory

## User Preferences
- Prefers dark mode in all applications
- Uses VS Code as primary editor
- Favorite language: Rust

## Project Context
- Working on nanobot project
- Migration from Python to Rust in progress
```

### HISTORY.md

Append chronological events:

```
Use write_file or edit_file to append:
~/.nanobot/memory/HISTORY.md

Format:
## 2024-01-15
- Started migration to Rust
- Implemented core agent loop
- Added skills system

## 2024-01-16
- Fixed compilation errors
- Added unit tests
```

## How to Read Memory

Use `read_file` to check existing memory:

```
read_file: ~/.nanobot/memory/MEMORY.md
read_file: ~/.nanobot/memory/HISTORY.md
```

## Best Practices

1. **Be Selective**: Only store truly important information
2. **Be Concise**: Keep entries brief and searchable
3. **Use Categories**: Organize MEMORY.md with clear sections
4. **Date Entries**: Always date HISTORY.md entries
5. **Review Regularly**: Periodically review and clean up memory

## Memory Window

The agent has a limited memory window (default: 50 messages). Long conversations will automatically consolidate older messages into MEMORY.md and HISTORY.md.

## Examples

### Storing User Preference
```
User: "I always want responses in Chinese"

Agent action:
edit_file:
  path: ~/.nanobot/memory/MEMORY.md
  old_string: "## User Preferences"
  new_string: "## User Preferences\n- Respond in Chinese (中文)"
```

### Logging Project Milestone
```
Agent action:
edit_file:
  path: ~/.nanobot/memory/HISTORY.md
  old_string: ""
  new_string: "\n## 2024-01-15\n- Completed skills system implementation\n- All unit tests passing"
```

## Important Notes

- Memory persists across sessions
- Memory is stored in plain Markdown files (human-readable)
- Use memory to avoid asking repetitive questions
- Memory helps maintain context in long-running projects
