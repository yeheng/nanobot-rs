---
summary: "Long-term Memory Management"
read_when:
  - Bootstrapping a workspace manually
---

# Long-term Memory Management (Memory Management ABI)

You can persist important state to disk using the `memorize` tool, and perform vector and tag-based retrieval via `memory_search`.
Storage space is precious. Do not write garbage to disk.

## Memory Partitions (Scenarios)

When calling `memorize`, you must accurately use one of the following lowercase enum values as the `scenario` parameter:

1. `profile`: User's persistent preferences, contact information, fixed environment variables. **Never decays.**
2. `active`: Current ongoing projects, context, unfinished tasks. Decays by default.
3. `knowledge`: New knowledge you've learned, code snippets, facts. Decays by default.
4. `decisions`: Architecture Decision Records (ADR), trade-off analysis. **Never decays.**
5. `episodes`: Specific past events, troubleshooting processes. Decays by default.
6. `reference`: External links, API documentation indexes. **Never decays.**

## Memory Writing Rules

- Each time you use `memorize`, you must provide at least 2 high signal-to-noise `tags`.
- If the user tells you "remember this", default to writing to `knowledge` or `profile`.
- The `title` must be brief and descriptive (similar to a Git commit message).
- **Always read before writing** — use `memory_search` or `read_file` first to avoid duplicates.

## Three-Layer Architecture

| Layer | Storage | Purpose |
|-------|---------|---------|
| Session Events | SQLite (`~/.gasket/gasket.db`) | Ephemeral conversation state, auto-compacted |
| Working Memory | In-memory + SQLite | Three-phase context injection per request |
| Long-Term Memory | Markdown files (`~/.gasket/memory/`) + SQLite index | Persistent user-curated knowledge |

## Long-Term Memory Storage

```
~/.gasket/memory/          ← Markdown files (SSOT, human-readable)
├── profile/               # NEVER decays
├── active/                # Decays if unused
├── knowledge/             # Decays if unused
├── decisions/             # NEVER decays
├── episodes/              # Decays if unused
├── reference/             # NEVER decays
└── .history/              # Versioned backups
```

## Frequency Lifecycle

Memories have access-frequency tiers affecting retention priority:

- **Hot** → **Warm** (no access 7d) → **Cold** (no access 30d) → **Archived** (no access 90d)
- Exempt scenarios (`profile`, `decisions`, `reference`) never decay.
- `frequency`, `access_count`, `last_accessed` are runtime state stored only in SQLite, never in Markdown files.

## Context Loading

Memory is injected into agent context via three phases (total cap: 4000 tokens):

1. **Bootstrap**: All `profile` + `active` hot/warm memories
2. **Scenario**: Current scenario hot + tag-matched warm memories
3. **On-demand**: Semantic/tag search to fill remaining budget

Sort priority: exempt scenarios first → higher frequency → higher similarity.
