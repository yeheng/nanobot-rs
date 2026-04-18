---
summary: "Long-term Memory Management"
read_when:
  - Bootstrapping a workspace manually
---

# Long-term Memory Management (Memory Management ABI)

You can persist important state to disk using the `memorize` tool, and perform vector and tag-based retrieval via `memory_search`.
Storage space is precious. Do not write garbage to disk.

## Memory vs Skill — Know the Boundary

| | **Memory** | **Skill** |
|---|---|---|
| Answers | "What" (facts, preferences, events) | "How" (procedures, workflows, SOPs) |
| Stored in | `~/.gasket/memory/<scenario>/` | `workspace/skills/<name>.md` |
| Format | YAML frontmatter + Markdown | YAML frontmatter + Markdown |
| Type field | `note` (default) | `skill` |
| Trigger | Always loaded if relevant | Loaded on demand via `read_file` or `skill_view` |

**Rule of thumb**: If it's a fact about the user or the world → Memory. If it's a reusable procedure with steps and pitfalls → Skill.

When writing a Skill-level procedure to memory, pass `"memory_type": "skill"` to the `memorize` tool. The system prioritizes skill-type memories during context loading — they are treated as actionable procedural knowledge and ranked above plain facts.

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
- Use `memory_type: "skill"` when the content is a reusable procedure (steps, pitfalls, verification). Default is `"note"` for facts.
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

1. **Bootstrap** (~1500 tokens): All `profile` + `active` hot/warm memories
2. **Scenario** (~1500 tokens): Current scenario hot + tag-matched warm memories
3. **On-demand** (~1000 tokens): Semantic/tag search to fill remaining budget

**Architecture Note**: Loaded memories are injected as a **User Message** (not appended to the System Prompt). This preserves Prompt Cache on Anthropic and similar providers, because the System Prompt stays static across turns while dynamic memory content varies per request. For long sessions, this reduces API costs by 90%+.

Sort priority: exempt scenarios first → skill-type memories → higher frequency → higher similarity.
