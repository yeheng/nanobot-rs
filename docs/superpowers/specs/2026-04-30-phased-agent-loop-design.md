# Phased Agent Loop — Design Spec

**Date**: 2026-04-30
**Status**: Draft
**Scope**: Engine-level phased execution (Research → Planning → Execute → Review) with cross-session learning

---

## 1. Overview

Replace the current open-ended LLM loop with a phased execution model: **Research → Planning → Execute → Review → Done**. The Research phase is engine-enforced (auto-search wiki + history on every user message); subsequent phases are LLM-driven via a `phase_transition` tool.

**Two learning loops:**
- **In-session**: Research gathers context → Planning structures approach → Execute acts → Review extracts learnings
- **Cross-session**: Review writes knowledge into wiki; next session's Research automatically retrieves it

---

## 2. Architecture

### 2.1 New Components

```
gasket/engine/src/kernel/
├── phased_executor.rs    # Phase state machine + orchestration (wraps SteppableExecutor)
├── research_context.rs   # Auto-search aggregation + research sub-loop
├── steppable_executor.rs # UNCHANGED — pure step logic, no phase awareness
├── kernel_executor.rs    # UNCHANGED — PhasedExecutor replaces it at call site, not by modification
```

**Design decision:** `SteppableExecutor` remains a pure, stateless component. `PhasedExecutor` wraps it and manages phase-specific behavior externally — injecting filtered tool sets, phase-aware system messages, and intercepting `phase_transition` tool results. This preserves the kernel's purity and minimizes change risk.

### 2.2 Phase State Machine

```
enum AgentPhase {
    Research,   // Engine-enforced: auto-search → retrieval sub-loop → user clarification
    Planning,   // LLM-driven: create plan based on research findings
    Execute,    // LLM-driven: standard tool execution (current SteppableExecutor behavior)
    Review,     // LLM-driven: review results → extract learnings → write wiki
    Done,       // Terminal
}
```

### 2.3 Phase Transitions

| Current | Targets | Trigger |
|---------|---------|---------|
| Research | Planning, Execute | `phase_transition` tool |
| Planning | Execute | `phase_transition` tool |
| Execute | Review, Done | `phase_transition` tool |
| Review | Done | `phase_transition` tool |

- LLM can skip phases (e.g., Research → Execute → Done for simple queries)
- One exception: user message ALWAYS triggers engine-enforced Research startup
- Max iterations per phase: Research=5, Planning=3, Execute=unlimited, Review=3

**Iteration limits vs global max_iterations:** Phase limits are per-phase counters, independent of the global `KernelConfig::max_iterations` (default: 100). PhasedExecutor tracks a cross-phase cumulative counter. If the cumulative total reaches `max_iterations`, the engine forces transition to Done regardless of current phase. This provides a safety valve while allowing Execute to run without per-phase cap.

### 2.4 Tool Sets Per Phase

| Phase | Available Tools |
|-------|----------------|
| Research | `wiki_search`, `wiki_read`, `history_search`, `query_history`, `phase_transition` |
| Planning | `create_plan`, `phase_transition` + read-only tools |
| Execute | Full tool set (current behavior) |
| Review | `wiki_write`, `wiki_delete`, `evolution`, `phase_transition` + read-only tools |

### 2.5 Tool Filtering Mechanism

**Strategy:** Filter at request-building time, not at execution time.

`RequestHandler` currently passes all registered tools via `tools.get_definitions()`. With phased execution, `PhasedExecutor` wraps the tool set with a phase-aware filter before each `step()` call:

```rust
/// Phase-aware tool filter — wraps ToolRegistry, exposing only tools
/// valid for the current phase.
pub struct PhasedToolSet {
    registry: Arc<ToolRegistry>,
    allowed: Vec<String>,
}

impl PhasedToolSet {
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry
            .get_definitions()
            .into_iter()
            .filter(|def| self.allowed.contains(&def.function.name))
            .collect()
    }
}
```

**Why request-time filtering, not runtime interception:**
- LLM never sees unavailable tools, so it cannot attempt to call them — no "intercept and reject" logic needed
- Simpler error handling: no phase-violation edge cases
- Consistent with how `ToolRegistry` is used today (definitions → LLM → tool_calls → execution)

**Tool availability changes on phase transition:**
When `PhasedExecutor` detects a `phase_transition` tool call, it:
1. Validates the target phase is reachable from current phase
2. Switches internal phase state
3. Rebuilds the `PhasedToolSet` with the new phase's allowed tools
4. Injects the next phase's entry prompt

**`phase_transition` registration:** Registered in `ToolRegistry` like any other tool, but always included in `PhasedToolSet::allowed` regardless of phase (the tool is always visible; only its valid targets change per phase).

---

## 3. Research Phase (Engine-Enforced)

### 3.1 Auto-Search on User Message

On every user message, before the LLM sees it:

1. Engine calls `wiki_search(search_query, limit=5)` and — if the `embedding` feature is enabled — `history_search(search_query, limit=10)` in parallel
2. `search_query` is constructed by concatenating the last 2-3 user messages (when available), not just the latest one. This handles follow-up questions like "换个方案" where the latest message alone lacks context
3. Results are formatted and injected as a structured system message
4. LLM receives: auto-search context + user message

**Feature flag dependencies:**
- `wiki_search` is always available (Tantivy BM25, no extra feature)
- `history_search` requires the `embedding` feature flag (uses `RecallSearcher` for semantic similarity). When disabled, only wiki auto-search runs
- `query_history` (SQLite LIKE search) is always available as a fallback, but not used in auto-search (too noisy for unfiltered queries)

### 3.2 Injected Context Format

```
[Research Context — 自动检索]

## Wiki 相关页面 (N条)
- path/to/page (0.87): summary...

## 历史相关记录 (N条)
- [YYYY-MM-DD] user: "..."
- [YYYY-MM-DD] assistant: "..."

你可以用 wiki_read 查看完整页面，或 history_search 调整搜索方向。
需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。
```

### 3.3 Retrieval Sub-Loop

After receiving auto-search results, the LLM can:
- `wiki_read(path)` — read full wiki page content
- `wiki_search(query)` — search from a different angle
- `history_search(keywords)` — targeted history lookup (requires `embedding` feature)
- Respond to user to clarify intent

Each LLM response + tool result constitutes one sub-loop iteration. Loop continues until LLM calls `phase_transition`.

### 3.4 User Clarification Handling

When LLM responds with text (not tool calls), the engine sends the response to the user and waits. The user's response resumes the Research phase WITHOUT re-running auto-search — it's appended to the message list as a normal turn.

If the LLM response includes `phase_transition` in its tool calls, the phase transition proceeds.

**StepAction for nested interaction loops:**

Current `StepResult` uses `should_continue: bool`. For phased execution, this needs to be richer:

```rust
#[derive(Debug)]
pub enum StepAction {
    Continue,                          // Normal loop continuation
    WaitForUserInput,                  // LLM sent text, no tool calls — pause for user
    PhaseTransition { to: AgentPhase }, // LLM called phase_transition
}
```

`PhasedExecutor` checks each `StepResult`:
- `response.has_tool_calls()` where one is `phase_transition` → `PhaseTransition`
- `response.content` is present, no tool calls → `WaitForUserInput`
- Otherwise → `Continue`

This cleanly handles the Research sub-loop's user interaction without modifying `SteppableExecutor` — the state machine logic lives entirely in `PhasedExecutor`.

### 3.5 Guard Rails

**Tool filtering is the primary defense** (see §2.5): Research phase only exposes `wiki_search`, `wiki_read`, `history_search`, `query_history`, and `phase_transition`. The LLM physically cannot call `shell` or `write_file` because they don't appear in its tool definitions.

**Soft prompt at iteration limit:** After 5 iterations in Research, engine injects: "信息已足够，请调用 phase_transition 进入下一阶段"

**Hard fallback:** If LLM still does not call `phase_transition` within 2 additional iterations after the soft prompt (7 total), the engine forces a transition to Execute phase with `context_summary` annotated as "Research 达到迭代上限，由引擎强制推进"

---

## 4. Planning Phase

Entry prompt:
```
[Phase: Planning]

研究阶段收集到的上下文：
> {context_summary from Research}

基于以上信息和用户的需求，请制定执行计划。简单任务可以直接跳过此阶段。
制定完成后调用 phase_transition("execute") 进入执行。
```

- LLM produces a natural-language plan
- `create_plan` tool optionally available
- Max 3 iterations
- Can fast-forward: direct `phase_transition("execute")`

---

## 5. Execute Phase

Entry prompt:
```
[Phase: Execute]

执行你的计划。所有工具现在可用。
完成后调用 phase_transition("review") 进行复盘，或 phase_transition("done") 直接结束。
```

- Full tool access — identical to current SteppableExecutor behavior
- No iteration limit
- Fully compatible with existing subagent mechanism

---

## 6. Review Phase & Cross-Session Learning

### 6.1 Entry Prompt

```
[Phase: Review]

审视刚才的执行过程，回答三个问题：
1. 结果是否达到了用户的目标？
2. 这次对话中有哪些值得持久保存的知识？
3. 有哪些 wiki 页面应该创建或更新？

如果发现了持久知识，主动调用 wiki_write 写入。
完成后调用 phase_transition("done")。
```

### 6.2 Knowledge Extraction Paths

| Path | Action | Wiki Type |
|------|--------|-----------|
| New fact/preference | `wiki_write` with tags `["auto_learned"]` | topic / entity |
| Update existing page | `wiki_read` → append experience → `wiki_write` | topic / sop |
| No valuable knowledge | `phase_transition("done")` directly | — |

### 6.3 Relationship with EvolutionTool

| Dimension | EvolutionTool (existing) | Review Phase (new) |
|-----------|------------------------|-------------------|
| Timing | Background cron, async batch | End of conversation, real-time |
| Scope | All sessions, incremental scan | Current session only |
| Quality | Offline extraction, no context | In-context review by main LLM |
| Role | Safety net | Primary learning path |

Both coexist — Review phase does immediate high-quality extraction; EvolutionTool catches what's missed.

### 6.4 Dedup Coordination

**Problem:** Review and Evolution both write to wiki. Without coordination, the same knowledge is double-written.

**Strategy — shared slug-based dedup:**

1. Review phase writes pages with tag `"phase: review"`. Evolution writes with tag `"auto_learned"`. This makes provenance traceable.
2. Before writing, both use the same `slugify(title)` dedup check against existing wiki pages (same logic as `EvolutionTool::process_session` line 278-284)
3. Evolution's watermark mechanism already skips already-processed events — if Review wrote a page during session N, Evolution's next scan of session N will still process the events but skip writing the duplicate slug

**No cross-coordination needed at runtime** — the slug-based dedup is sufficient because:
- Same title → same slug → second writer skips
- Different titles about the same topic → both coexist (acceptable, wiki pages can overlap)

### 6.5 Review Knowledge Quality Gate

To prevent LLM from flooding wiki with low-value pages during Review:

1. **Confidence threshold:** When LLM calls `wiki_write` during Review, it must include a `confidence` tag (e.g., `["phase: review", "confidence: 0.8"]`). Wiki pages with confidence < 0.7 are marked for periodic cleanup.
2. **Limit per session:** Review phase can write at most 3 wiki pages per session. After 3, the engine injects: "已达到单次 Review 写入上限，剩余知识将由后台 Evolution 处理"
3. **Write requires summary:** Each `wiki_write` during Review must have a one-line `context_summary` in the content explaining why this knowledge is durable

### 6.6 Cross-Session Learning Flow

```
Session N                          Session N+1
┌──────────┐                      ┌──────────┐
│ Research │──wiki_search──→      │ Research │  ← auto-finds knowledge from Session N
│    ↓     │                      │    ↓     │
│ Execute  │                      │ Execute  │  ← avoids past mistakes
│    ↓     │                      │    ↓     │
│ Review ──┼──wiki_write──→       │ Review   │  ← continues accumulating
└──────────┘                      └──────────┘
     │                                  │
     └──────── wiki knowledge ──────────┘
            (SQLite + Tantivy)
```

---

## 7. phase_transition Tool

### 7.1 Definition

```json
{
  "name": "phase_transition",
  "description": "Transition to the next working phase.",
  "parameters": {
    "type": "object",
    "properties": {
      "phase": {
        "type": "string",
        "enum": ["planning", "execute", "review", "done"]
      },
      "context_summary": {
        "type": "string",
        "description": "Optional summary of findings for the next phase"
      }
    },
    "required": ["phase"]
  }
}
```

### 7.2 Behavior

- **Engine-internal tool**: not registered in the public ToolRegistry; attached only by PhasedExecutor
- **Valid targets are phase-dependent**: Research only exposes `["planning", "execute"]`, Planning exposes `["execute"]`, etc.
- **Tool result is intercepted by engine**: engine switches phase and injects new phase prompt; LLM never sees the raw tool output
- `context_summary` is **cumulative** — each phase's entry prompt receives all preceding phases' summaries concatenated, not just the immediately prior phase's

**Cumulative context format:**
```
[Phase: Execute]

## Collected Context
### Research (Research phase)
{research_summary}

### Plan (Planning phase)
{planning_summary}

执行你的计划。所有工具现在可用。
完成后调用 phase_transition("review") 进行复盘，或 phase_transition("done") 直接结束。
```

This prevents information attenuation across multi-step transitions (Research → Planning → Execute).

---

## 8. Frontend Integration

### 8.1 Phase Indicator

Add an optional `phase` field to WebSocket stream events. When using PhasedExecutor, the frontend displays a phase chip/badge:

```
[Research] → [Planning] → [Execute] → [Review] → ✓
```

### 8.2 PhaseTransition Event Schema

Extend `StreamEvent` enum with a new variant:

```rust
// In engine/src/kernel/stream.rs
pub enum StreamEvent {
    // ... existing variants ...

    /// Phase transition notification — lightweight, no content.
    /// Frontend uses this to update phase chip UI.
    PhaseTransition {
        from: String,  // "research" | "planning" | "execute" | "review"
        to: String,    // "research" | "planning" | "execute" | "review" | "done"
    },
}
```

**Wire format (JSON):**
```json
{
  "type": "phase_transition",
  "from": "research",
  "to": "execute"
}
```

**Frontend behavior:**
- On `PhaseTransition { to: "done" }`, replace phase chip with checkmark ✓
- Non-phased sessions never emit this event — backward compatible by default
- Fast queries flash through phases nearly imperceptibly

### 8.3 Transitions

- Phase transitions are pushed as lightweight events (no content)
- Default behavior: backward compatible — non-phased sessions show no phase UI
- Fast queries flash through phases nearly imperceptibly

---

## 9. Fast Path for Simple Queries

```
User: "现在几点了？"
  Research: auto-search → irrelevant → LLM calls phase_transition("execute")
  Execute: shell("date") → result → phase_transition("done")
  Done
```

The phased model imposes minimal overhead: simple queries skip Planning and Review, Execute runs exactly as today.

---

## 10. Implementation Order

1. **PhasedToolSet + tool filtering** — request-time tool filtering, foundation for everything
2. **PhasedExecutor state machine** — wraps SteppableExecutor, manages phase state, StepAction handling
3. **phase_transition tool** — internal tool with engine-side phase switching (now testable with filtering)
4. **Research auto-search** — automatic wiki_search + history_search injection
5. **Research sub-loop** — retrieval iteration with user clarification (WaitForUserInput)
6. **Planning / Review prompts** — phase-aware system prompts
7. **Review quality gate + dedup** — confidence threshold, write limit, slug dedup
8. **Frontend phase indicator** — StreamEvent::PhaseTransition + UI badge
9. **Integration gate** — config flag to opt-in to phased mode (backward compatible)

**Rationale:** Tool filtering (#1) is the prerequisite for all phase-specific behavior — without it, testing any phase logic is meaningless because LLM can call any tool. PhasedExecutor (#2) provides the state machine that everything else plugs into.

## 11. Backward Compatibility

### 11.1 Configuration Flag

Add to `KernelConfig`:

```rust
pub struct KernelConfig {
    // ... existing fields ...
    /// Enable phased execution (Research → Planning → Execute → Review → Done).
    /// When false (default), behavior is identical to current SteppableExecutor loop.
    pub phased_execution: bool,
}
```

### 11.2 Call Site Integration

The session layer (which currently calls `kernel::execute_streaming()`) checks this flag:

```rust
if ctx.config.phased_execution {
    PhasedExecutor::new(ctx).run(messages, event_tx).await
} else {
    kernel::execute_streaming(&ctx, messages, event_tx).await
}
```

### 11.3 YAML Configuration

In `~/.gasket/config.yaml`, under the agent section:

```yaml
agents:
  default:
    model: openrouter/anthropic/claude-4.5-sonnet
    phased_execution: true  # opt-in, default false
```

### 11.4 Compatibility Guarantees

- When `phased_execution: false`: zero behavioral change, no new code paths executed
- `StreamEvent::PhaseTransition` is never emitted — frontend sees no phase UI
- All existing tools remain registered and available
- `SteppableExecutor` is unchanged — non-phased mode uses identical code paths

## 12. Error Handling

### 12.1 Phase Iteration Exhaustion

When a phase reaches its iteration limit and LLM has not called `phase_transition`:

| Phase | Soft limit | Hard limit (after soft) | Forced transition |
|-------|-----------|------------------------|-------------------|
| Research | 5 | 7 | → Execute |
| Planning | 3 | 5 | → Execute |
| Execute | — | — | Governed by global `max_iterations` |
| Review | 3 | 5 | → Done |

At soft limit: inject prompt encouraging transition.
At hard limit: engine forces transition with annotated `context_summary`.

### 12.2 Global Iteration Exhaustion

Cross-phase cumulative counter reaches `max_iterations`:
- Engine forces immediate transition to Done
- Final message to user: "达到最大迭代次数，任务执行被截断"
- `ExecutionResult` includes partial results and list of completed phases

### 12.3 Tool Execution Errors in Research

If a research tool (`wiki_search`, `history_search`) fails:
- Auto-search failure is non-fatal: inject "自动搜索未返回结果，请手动搜索或直接进入下一阶段"
- Tool call failures during sub-loop are returned to LLM normally (it can retry or transition)

### 12.4 Phase Transition Validation Errors

If LLM calls `phase_transition` with an invalid target (e.g., Research → Review):
- Return tool error: "无效的阶段转换: research → review。允许的目标: planning, execute"
- LLM can retry with valid target
