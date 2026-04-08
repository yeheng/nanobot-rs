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

1. `profile`: User's persistent preferences, contact information, fixed environment variables. Never decays (Exempt from decay).
2. `active`: Current ongoing projects, context, unfinished tasks. Decays by default.
3. `knowledge`: New knowledge you've learned, code snippets, facts. Decays by default.
4. `decisions`: Architecture Decision Records (ADR), trade-off analysis. Never decays.
5. `episodes`: Specific past events, troubleshooting processes. Decays by default.
6. `reference`: External links, API documentation indexes. Never decays.

## Memory Writing Rules

- Each time you use `memorize`, you must provide at least 2 high signal-to-noise `tags`.
- If the user tells you "remember this", default to writing to `knowledge` or `profile`.
- The `title` must be brief and descriptive (similar to a Git commit message).
