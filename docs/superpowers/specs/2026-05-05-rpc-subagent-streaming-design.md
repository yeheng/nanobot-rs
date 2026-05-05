# RPC Subagent Streaming + Dev Workflow Plugin — Design

**Date:** 2026-05-05
**Status:** Draft → pending user review

## Problem Statement

The `SubagentSpawnHandler` in `gasket/engine/src/plugin/dispatcher/subagent.rs` calls `spawner.spawn(...).await`, which is the **blocking, non-streaming** entry point. As a result, every `subagent/spawn` RPC made by a JSON-RPC plugin produces zero `StreamEvent`s on the user-facing WebSocket — no `Thinking`, no per-token `Content`, no tool start/end. For a plugin that drives a multi-minute multi-subagent workflow, the UI looks frozen until the very end.

The native `SpawnTool` (`engine/src/tools/spawn.rs`) already does this correctly via `spawn_with_stream` + `spawn_common::spawn_event_forwarder`. The fix is to make the RPC handler use the same primitives.

A secondary goal is to validate the fix end-to-end by shipping a real long-running plugin: a Research → Plan → Implement → Review workflow written in Python.

## Goals

1. **Task 1** — Stream `StreamEvent`s from RPC-spawned subagents to the WebSocket in real time, matching the behavior of the native `SpawnTool`.
2. **Task 2** — Provide a minimal Python SDK (`gasket_sdk.py`) so plugins don't reimplement JSON-RPC stdio plumbing.
3. **Task 3** — Ship a `dev_workflow` plugin that proves the end-to-end pipeline: multi-subagent orchestration with per-node model selection and a bounded review loop.

## Non-Goals

- No changes to `daemon.rs` timeout semantics. The known `idle_timeout_ms` field-doubling (per-call timeout + idle GC) is left as-is; long-running plugins use a high `timeout_secs` in their manifest.
- No engine-level model profile/alias system. Model IDs are passed through as opaque strings (`provider/model` form) and selected by plugin authors via YAML inputs.
- No changes to the `SpawnRequest` / `SpawnResponse` JSON shape (backward compatible).
- No introduction of dedicated exception types in the Python SDK; `RuntimeError` is sufficient for single-file plugins.

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│              gasket tool execute dev_workflow {...}                │
└──────────────────────────────────────────────────────────────────┘
                               │ args: {task, max_iterations, reasoner_model, coder_model}
                               ▼
┌──────────────────────────────────────────────────────────────────┐
│  Plugin Daemon (Python, workflows/dev.py)                          │
│  ┌─────────────────────────────────────────────────────────┐      │
│  │ from gasket_sdk import GasketPlugin                     │      │
│  │ plugin.spawn_subagent(task, model)  →  blocks on JSON-RPC│     │
│  └─────────────────────────────────────────────────────────┘      │
└──────────────────────────────────────────────────────────────────┘
                  │ stdin: {jsonrpc:"2.0", method:"subagent/spawn", id:N}
                  ▼
┌──────────────────────────────────────────────────────────────────┐
│  RpcDispatcher → SubagentSpawnHandler   ★ Task 1 fix              │
│                                                                   │
│  spawn_with_stream(task, model_id)                                │
│    ├──→ event_rx ──→ spawn_event_forwarder() ──┐                  │
│    └──→ result_rx ──── await ─────────┐         │                 │
│                                       ▼         ▼                 │
│                                   SpawnResponse  outbound_tx      │
└──────────────────────────────────────────────────────────────────┘
                                                  │
                                                  ▼
                                         Frontend WebSocket:
                                         live thinking + tokens
```

**Key invariants:**
- The `SpawnRequest` and `SpawnResponse` JSON shapes are unchanged.
- Engine-side change is contained in **two files**: `subagent.rs` (logic) and `tools/mod.rs` (visibility).
- Long-running plugins set `timeout_secs` per-plugin in YAML; `daemon.rs` is not modified.

---

## Task 1 — `SubagentSpawnHandler` Streaming Fix

**File:** `gasket/engine/src/plugin/dispatcher/subagent.rs`

**Approach (Option B — reuse `spawn_event_forwarder`):**

Replace the single `spawner.spawn(...).await` call with the streaming variant, then delegate event forwarding to the existing helper that the native `SpawnTool` already uses. This avoids duplicating the `StreamEventKind` → `ChatEvent` match.

**Pseudocode (final shape):**

```rust
// Replaces lines 56-59 of current subagent.rs
let (subagent_id, event_rx, result_rx, _cancel) = spawner
    .spawn_with_stream(request.task.clone(), request.model_id.clone())
    .await
    .map_err(|e| RpcError::internal_error(format!("spawn failed: {}", e)))?;

// Notify frontend that the subagent has started (mirrors spawn.rs behavior)
let _ = ctx.engine.outbound_tx.send(
    OutboundMessage::with_ws_message(
        ctx.engine.session_key.channel.clone(),
        ctx.engine.session_key.chat_id.clone(),
        ChatEvent::subagent_started(subagent_id.clone(), request.task.clone(), 0),
    )
).await;

// Forward StreamEvents → ChatEvents in the background
let _forward_handle = crate::tools::spawn_common::spawn_event_forwarder(
    subagent_id.clone(),
    event_rx,
    ctx.engine.session_key.clone(),
    ctx.engine.outbound_tx.clone(),
);

// Block on the final result
let result = result_rx.await
    .map_err(|e| RpcError::internal_error(format!("subagent dropped: {}", e)))?;

// Build SpawnResponse exactly as before — JSON shape unchanged
let response = SpawnResponse {
    id: result.id,
    task: result.task,
    content: result.response.content,
    model: result.model,
};
```

**Visibility change (`tools/mod.rs`):**

```rust
// Before:
mod spawn_common;
// After:
pub(crate) mod spawn_common;
```

The function `spawn_event_forwarder` is already `pub`; only the parent module needs to be reachable from the `crate::plugin::dispatcher::subagent` path.

**Test (added to `subagent.rs#[cfg(test)]`):**

A `MockStreamingSpawner` whose `spawn_with_stream` returns prepared `event_rx` and `result_rx`. The test pushes a `Thinking` event and a `Content` event into `event_rx`, completes `result_rx`, then drains the `outbound_rx` and asserts:
- A `SubagentStarted` `ChatEvent` was sent.
- A `SubagentThinking` `ChatEvent` was sent with the matching content.
- A `SubagentContent` `ChatEvent` was sent.
- The handler returned the correct `SpawnResponse` JSON.

**Acceptance criteria:**
- The new test passes.
- The existing `subagent.rs` tests continue to pass.
- Manually verified: a plugin invoking `subagent/spawn` produces visible thinking + typewriter output on the frontend, just like the native `SpawnTool`.

---

## Task 2 — Python `gasket_sdk`

**File (new):** `workspace/plugins/gasket_sdk.py`

**Surface:**

```python
class GasketPlugin:
    def get_args(self) -> dict: ...
    def return_result(self, result: dict) -> None: ...
    def return_error(self, code: int, message: str) -> None: ...
    def spawn_subagent(self, task: str, model: Optional[str] = None) -> str: ...
    def llm_chat(self, model: str, messages: list, **kwargs) -> dict: ...
```

**Design rules:**
- One file, no package, no third-party dependencies.
- Single-threaded request-response. `_call` assumes responses arrive in request order. No id-based dispatch table — that's YAGNI for current plugins.
- No daemon-style `while True` loop in the SDK. The plugin script is a one-shot script: `get_args` → work → `return_result` → exit. Daemon reuse is the Rust runner's concern.
- All errors raise `RuntimeError`. No custom exception hierarchy.

**Implementation sketch:** see Section 3 of the design conversation. Approximately 80 lines.

**Acceptance criteria:**
- `from gasket_sdk import GasketPlugin` works from a sibling directory (`workflows/dev.py` uses `sys.path.insert(0, ...)` to import).
- The existing `jsonrpc_ping/ping.py` can be rewritten to use the SDK in ≤ 15 lines and still passes its existing test (regression check).

---

## Task 3 — `dev_workflow` Plugin

**Files (new):**
- `workspace/plugins/dev_workflow.yaml`
- `workspace/plugins/workflows/dev.py`

### YAML manifest (key fields)

```yaml
name: "dev_workflow"
protocol: json_rpc
parameters:
  type: object
  properties:
    task: { type: string }
    max_iterations: { type: integer, default: 3 }
    reasoner_model: { type: string }
    coder_model: { type: string }
  required: ["task"]
runtime:
  command: "python3"
  args: ["workflows/dev.py"]
  timeout_secs: 1200
  env:
    PYTHONUNBUFFERED: "1"
permissions:
  - subagent_spawn
```

**Notes:**
- `timeout_secs: 1200` is the upper bound for the entire workflow (research + plan + N×(implement + review)).
- `reasoner_model` / `coder_model` are optional; missing fields are passed as `None` and the engine uses its default model.

### Python flow

1. `args = plugin.get_args()` — wait for the `initialize` request.
2. Phase **Research** — `plugin.spawn_subagent(...research prompt..., model=reasoner_model)`.
3. Phase **Plan** — same pattern, prompt includes the research output.
4. Phase **Implement → Review loop** — bounded by `max_iterations`:
   - Implement with `coder_model`, prompt includes the plan and the previous attempt.
   - Review with `reasoner_model`, prompt forces strict JSON: `{"verdict": "PASS"|"FAIL", "reason": "..."}`.
   - `parse_verdict` tolerates `\`\`\`json` fences but treats unparseable output as `FAIL`.
   - On `FAIL`, append the reason to the plan (do not replace the plan) and loop.
   - On `PASS`, break.
5. `plugin.return_result({"final_code": code, "passed": bool, "iterations_used": int, "last_review_reason": str})`.

**Why JSON verdict, not string match:**
The original `"PASS" in review` check has known false positives (e.g., a reviewer saying "no PASS yet" matches). Strict JSON makes the contract verifiable; the fence-tolerance is the one concession to LLM misbehavior.

**Why append-to-plan, not replace:**
Replacing the plan with raw review text loses the original specification. The implement subagent on the next iteration should still see the original goals plus the new feedback.

**Acceptance criteria:**
- `gasket tool execute dev_workflow '{"task": "write a python snake game"}'` runs to completion.
- During execution, the frontend shows live `SubagentThinking` / `SubagentContent` for at least 4 distinct subagent IDs (research, plan, implement#1, review#1) when the first attempt PASSes; more if it doesn't.
- When the reviewer returns `FAIL`, the next iteration sees the appended feedback.
- The returned JSON contains all four fields: `final_code`, `passed`, `iterations_used`, `last_review_reason`.

---

## Risks & Tradeoffs

| Risk | Likelihood | Mitigation |
|---|---|---|
| `spawn_event_forwarder` panics if `outbound_tx` is closed mid-flight | Low | Existing helper already swallows send errors via `let _ =`; same blast radius as `SpawnTool` today. |
| Reviewer never returns parseable JSON, all iterations FAIL | Medium | `passed=false` is returned with the last code; caller decides what to do. Not a hang. |
| `timeout_secs: 1200` plugin holds a daemon slot for 20 minutes after exiting | Medium | This is the existing `idle_timeout_ms` doubling behavior; called out as non-goal. Acceptable for the workflow's typical run cadence. |
| Plugin authors hard-code provider-specific model strings in YAML | Low | This is intentional. Engine-level profile aliases are explicitly out of scope. |

## Files Touched

| File | Type | Lines |
|---|---|---|
| `gasket/engine/src/plugin/dispatcher/subagent.rs` | modify | ~30 changed |
| `gasket/engine/src/tools/mod.rs` | modify | 1 changed (`mod` → `pub(crate) mod`) |
| `workspace/plugins/gasket_sdk.py` | new | ~80 |
| `workspace/plugins/dev_workflow.yaml` | new | ~25 |
| `workspace/plugins/workflows/dev.py` | new | ~90 |

## Open Questions

None. All decisions resolved during brainstorming:
- Scope: full execution of all three tasks.
- Model selection: per-plugin YAML inputs, opaque pass-through.
- Timeout: per-plugin `timeout_secs`, no engine-level changes.
- Loop termination: strict JSON verdict + best-effort return on max iterations.
- Forwarder reuse: Option B (reuse `spawn_event_forwarder`) over inline match or `ToolDelegateHandler`.
