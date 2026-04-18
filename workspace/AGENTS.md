---
summary: "Subagent tooling guidelines for gasket"
read_when:
  - Bootstrapping a workspace manually
---

# Process & Subagent Scheduling

You have two subagent tools: `spawn` (blocking single execution) and `spawn_parallel` (concurrent execution). Both delegate work to subagents that run the same engine with a fresh context.

## Tool Reference

### `spawn` — Single Task
- **Args**: `{ "task": "description", "model_id?": "provider/model" }`
- **Use when**: A task needs deep focus before continuing (e.g., "analyze this 500-line function")
- **Model ID**: Optional. If omitted, uses the default model. Must be a valid `provider/model` string from config (e.g., `"openrouter/anthropic/claude-4.5-sonnet"`), not abstract names like `"reasoner"`.

### `spawn_parallel` — Concurrent Tasks
- **Args**: `{ "tasks": ["task1", "task2", ...] }` or `[{ "task": "...", "model_id?": "..." }]`
- **Max**: **10 tasks per call**
- **Use when**: Multiple independent lookups (e.g., search 3 docs + read 2 files)
- **Important**: After `spawn_parallel`, aggregate results yourself. Do not re-spawn.

## Scheduling Rules

1. **I/O-Bound Parallelism**: Multiple web searches, file reads, or page scrapes → `spawn_parallel`.
2. **CPU/Reasoning Bound**: Deep analysis, math, architecture review → `spawn` with a strong model.
3. **No Nested Spawning**: A subagent should not call `spawn` or `spawn_parallel`. If a subagent reports it needs more delegation, you did the decomposition wrong — fix it at your level.
4. **Data Aggregation**: After `spawn_parallel`, your only job is to merge results (deduplicate, rank, summarize) and present to the user.
