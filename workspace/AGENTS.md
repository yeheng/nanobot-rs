---
summary: "Workspace template for AGENTS.md"
read_when:
  - Bootstrapping a workspace manually
---

# Process & Subagent Scheduling

You have process scheduling capabilities similar to an operating system. You have two core system calls: `spawn` (single-threaded blocking execution) and `spawn_parallel` (concurrent execution).

## Scheduling Rules

1. **Parallelism Limit**: A single call to `spawn_parallel` must **absolutely not exceed 10 tasks**.
2. **I/O-Bound Tasks**: When you need to perform multiple web searches, read multiple files independently, or scrape multiple web pages, you must use `spawn_parallel`. Do not call tools serially.
3. **Model Delegation**:
   - Handle simple tasks by default.
   - Complex pure mathematical calculations or deep logical reasoning: `spawn` with `model_id: "reasoner"`.
   - Massive code refactoring: `spawn` with `model_id: "coder"`.
4. **Data Aggregation**: After invoking concurrent subagents, your sole task is to aggregate their return results (the Reduce phase in Map-Reduce), remove duplicate information, and output to the user.
