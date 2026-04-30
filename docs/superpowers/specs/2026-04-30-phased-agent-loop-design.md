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
├── phased_executor.rs    # Phase state machine + orchestration
├── research_context.rs   # Auto-search aggregation + research sub-loop
├── steppable_executor.rs # MODIFIED — accept phase-aware prompts
├── kernel_executor.rs    # MODIFIED — optionally wrap with PhasedExecutor
```

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

### 2.4 Tool Sets Per Phase

| Phase | Available Tools |
|-------|----------------|
| Research | `wiki_search`, `wiki_read`, `history_search`, `query_history`, `phase_transition` |
| Planning | `create_plan`, `phase_transition` + read-only tools |
| Execute | Full tool set (current behavior) |
| Review | `wiki_write`, `wiki_delete`, `evolution`, `phase_transition` + read-only tools |

---

## 3. Research Phase (Engine-Enforced)

### 3.1 Auto-Search on User Message

On every user message, before the LLM sees it:

1. Engine calls `wiki_search(user_query, limit=5)` and `history_search(user_query, limit=10)` in parallel, where `user_query` is the user's raw message text (no preprocessing — BM25 handles natural language well)
2. Results are formatted and injected as a structured system message
3. LLM receives: auto-search context + user message

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
- `history_search(keywords)` — targeted history lookup
- Respond to user to clarify intent

Each LLM response + tool result constitutes one sub-loop iteration. Loop continues until LLM calls `phase_transition`.

### 3.4 User Clarification Handling

When LLM responds with text (not tool calls), the engine sends the response to the user and waits. The user's response resumes the Research phase WITHOUT re-running auto-search — it's appended to the message list as a normal turn.

If the LLM response includes `phase_transition` in its tool calls, the phase transition proceeds.

### 3.5 Guard Rails

- Non-research tool calls (e.g., `shell`) during Research are intercepted: "请在 Research 阶段先完成信息收集"
- After 5 iterations, engine injects: "信息已足够，请调用 phase_transition 进入下一阶段"

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

### 6.4 Cross-Session Learning Flow

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
- `context_summary` is passed to the next phase's entry prompt

---

## 8. Frontend Integration

### 8.1 Phase Indicator

Add an optional `phase` field to WebSocket stream events. When using PhasedExecutor, the frontend displays a phase chip/badge:

```
[Research] → [Planning] → [Execute] → [Review] → ✓
```

### 8.2 Transitions

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

1. **phase_transition tool** — internal tool with engine-side phase switching
2. **PhasedExecutor** — state machine wrapping SteppableExecutor
3. **Research auto-search** — automatic wiki_search + history_search injection
4. **Research sub-loop** — retrieval iteration with user clarification
5. **Planning / Review prompts** — phase-aware system prompts
6. **Frontend phase indicator** — WebSocket event field + UI badge
7. **Integration gate** — config flag to opt-in to phased mode (backward compatible)
