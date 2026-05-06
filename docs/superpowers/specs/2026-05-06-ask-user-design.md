# Ask-User: Plugin/Agent Pause-and-Wait-for-User-Reply — Design

**Date:** 2026-05-06
**Status:** Draft → pending user review

## Problem Statement

Gasket today is a **one-way batch model**: an `InboundMessage` enters via a channel, `AgentSession::process_direct()` spins up a single LLM turn, tools execute synchronously, and a final response is emitted. While a plugin / subagent / tool is running, the engine cannot suspend execution to ask the user a clarifying question and resume once the user replies. Concretely:

- `tool_registry.execute(...)` is `await`-blocked inside the kernel loop, which holds the session's processing future. The next `InboundMessage` for the same `SessionKey` would start a *new* `process_direct` call rather than be routed back to the in-flight tool.
- All plugin → engine RPCs (`subagent/spawn`, `llm/chat`, `message/send`, `wiki/*`) are pull-only: plugins request, engine responds. There is no mechanism for the engine to *push* "the next user message" back into a paused plugin.
- `OutboundMessage` is a single-direction handoff with no reply-correlation id. Even if a plugin emits a question, the engine has no way to associate the user's subsequent inbound with that specific question.

**Goal:** allow workflow / plugin / subagent / main-agent code to **ask the user a question and synchronously block on the answer**, surfacing the reply as a tool result.

## Bounded Decisions (locked from brainstorming)

| Decision | Choice | Rationale |
|---|---|---|
| Wait durability | **Medium** — minutes to hours, in-memory, session-lifetime only | Avoids storage / cross-process recovery complexity. |
| Capability owner | **Unified `ask_user` tool** in `ToolRegistry` | Single primitive serves main agent, subagents, and plugins; reuses `ToolDelegateHandler` for plugin RPC. |
| History semantics | **Pure tool-call semantics** — answer enters only as `tool_result` | No double-bookkeeping; no ambiguous "is this a user turn or an answer?" state. |
| Timeout | **Required parameter** — caller must pass `timeout_secs`; on expiry returns `Err(Timeout)` | Forces explicit responsibility; no magic defaults. |
| Concurrency | **Single pending ask per `SessionKey`** | Stack-style nesting (plugin → subagent → ask) remains legal because at any instant only the deepest frame is active; concurrent asks would require question-id routing, which plain-text channels can't carry. |
| Inbound during pending | **All inbound captured as the answer** | Engine ships no magic-word list (`/cancel`, etc.); callers parse the answer themselves. |
| Return shape | **Structured object** `{content, sender_id, channel, timestamp, media?}` | Forward-compatible; reuses `gasket_types::events::Media`. |

## Goals

1. **Goal 1** — Add a built-in `ask_user` tool to the engine `ToolRegistry` that blocks on user reply with a caller-specified timeout.
2. **Goal 2** — Add a `PendingAskRegistry` owned by `AgentSession` and a thin `handle_inbound` entry point that routes inbound to a pending ask if one exists, otherwise falls through to the existing `process_direct` path.
3. **Goal 3** — Expose the capability to plugins via a new `Permission::UserAsk` and `user/ask` RPC, delegating to the same `ask_user` tool.
4. **Goal 4** — Update `gasket_sdk.py` with an `ask_user` helper.

## Non-Goals

- No cross-process / cross-restart persistence. Engine restart drops all pending asks (callers see `Err(Cancelled)`).
- No multi-pending routing on a single `SessionKey`. Concurrent asks return `Err(AlreadyPending)`.
- No magic-word handling (`/cancel`, etc.). The engine forwards whatever the user types as the answer.
- No structured answer schemas (multiple-choice, JSON-Schema validation). Plain text + optional media only.
- No new `ChatEvent::Question` variant for the WebSocket frontend in this iteration. Plain `OutboundMessage` carries the prompt.
- No changes to broker / types crate semantics. Routing logic stays inside engine.

## Architecture

```
                  inbound from any channel
                              │
                              ▼
                  ┌──────────────────────────────┐
                  │ AgentSession.handle_inbound()│  ◀── new thin entry
                  └──────┬───────────────────────┘
                         │
            ┌────────────┴────────────┐
            ▼                         ▼
  PendingAskRegistry          (no pending → fall through)
  .try_fulfill(key, msg)              │
            │                         ▼
   hit:  oneshot.send(answer)   process_direct() → kernel loop
   miss: return Err(msg)              │
                                      ▼
                              tool call: ask_user
                                      │
                                      ▼
                  AskUserTool::execute
                  ├─ register slot in PendingAskRegistry
                  │  (rejects second register on same key)
                  ├─ outbound_tx.send(prompt as OutboundMessage)
                  ├─ tokio::select! { rx.await | sleep(timeout) }
                  └─ AskAnswer JSON  /  Err(Timeout|AlreadyPending|Cancelled)
                                      │
                                      ▼
                            kernel sees tool_result, next LLM turn
```

**Key invariants:**
- `PendingAskRegistry` is owned by `AgentSession`. Tools obtain it via `ToolContext.pending_asks` (new `Option<Arc<PendingAskRegistry>>` field).
- A `SessionKey` slot is occupied **iff** a future is awaiting `oneshot::Receiver`. Every successful `register` is paired with exactly one of `try_fulfill` (matched by inbound) or `cancel` (timeout / abort).
- Plugin path **automatically inherits** the registry: `ToolDelegateHandler` (`engine/src/plugin/dispatcher/mod.rs:248-269`) builds a `ToolContext` from `EngineHandle`; once `pending_asks` is added to `EngineHandle`, plugins reach `ask_user` through the same code path as the main agent.
- Kernel / LLM loop is **unchanged**. From its viewpoint, `ask_user` is just another tool whose future may take a long time to resolve.

## Data Structures

```rust
// engine/src/session/pending_ask.rs

pub struct PendingAsk {
    pub ask_id: uuid::Uuid,
    pub session_key: SessionKey,
    pub prompt: String,
    pub deadline: std::time::Instant,
    answer_tx: tokio::sync::oneshot::Sender<AskAnswer>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AskAnswer {
    pub content: String,
    pub sender_id: String,
    pub channel: gasket_types::events::ChannelType,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub media: Option<gasket_types::events::Media>,
}

pub struct PendingAskRegistry {
    inner: std::sync::Arc<tokio::sync::Mutex<
        std::collections::HashMap<SessionKey, PendingAsk>
    >>,
}

#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("session {0:?} already has a pending ask")]
    AlreadyPending(SessionKey),
    #[error("ask timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("ask cancelled: session shutting down")]
    Cancelled,
}

impl PendingAskRegistry {
    pub fn new() -> Self;
    pub fn register(
        &self,
        key: SessionKey,
        prompt: String,
        timeout: std::time::Duration,
    ) -> Result<(uuid::Uuid, tokio::sync::oneshot::Receiver<AskAnswer>), AskError>;
    pub fn cancel(&self, key: &SessionKey, ask_id: uuid::Uuid);
    pub fn try_fulfill(
        &self,
        key: &SessionKey,
        msg: gasket_types::events::InboundMessage,
    ) -> Result<(), gasket_types::events::InboundMessage>;
}
```

**`ask_user` tool JSON Schema:**

```json
{
  "name": "ask_user",
  "description": "Ask the user a question and wait for their reply. Returns a JSON object with the user's answer.",
  "parameters": {
    "type": "object",
    "properties": {
      "prompt":       { "type": "string" },
      "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 86400 }
    },
    "required": ["prompt", "timeout_secs"]
  }
}
```

Returned `tool_result` body (JSON-encoded `AskAnswer`):

```json
{ "content": "...", "sender_id": "...", "channel": "telegram", "timestamp": "2026-05-06T12:34:56Z", "media": null }
```

## Inbound Routing & Concurrency Semantics

### `handle_inbound` thin entry

```rust
// engine/src/session/mod.rs

pub enum HandleOutcome {
    Consumed,                    // inbound went to a pending ask
    Replied(AgentResponse),      // inbound triggered a normal LLM turn
}

impl AgentSession {
    pub async fn handle_inbound(
        &self,
        msg: InboundMessage,
        tool_filter: Option<Vec<String>>,
    ) -> Result<HandleOutcome, AgentError> {
        let key = SessionKey::from(&msg);
        match self.pending_asks.try_fulfill(&key, msg) {
            Ok(()) => Ok(HandleOutcome::Consumed),
            Err(unmatched) => {
                let resp = self
                    .process_direct(&unmatched.content, &key, tool_filter)
                    .await?;
                Ok(HandleOutcome::Replied(resp))
            }
        }
    }
}
```

### Concurrency matrix

| Scenario | Behavior |
|---|---|
| Same `SessionKey` already has a pending ask, kernel calls `ask_user` again | `register` returns `Err(AlreadyPending)`. `AskUserTool::execute` surfaces it as `ToolError::ExecutionError("…already_pending…")`. The LLM sees the error and learns to retry / give up. |
| Stack-style nesting (plugin → subagent → ask_user) | Legal. Subagent reuses the same `SessionKey`; the plugin is *not* awaiting `ask_user` at that instant — it is awaiting `subagent/spawn`. Only the deepest active frame holds the slot. |
| Inbound arrives for a `SessionKey` with a pending ask | `try_fulfill` succeeds; `oneshot::Sender::send(AskAnswer)` wakes the tool future; slot is taken. |
| Pending times out | Tool's `tokio::select!` chooses sleep branch; tool calls `cancel(key, ask_id)`; returns `Err(Timeout)`. |
| `AgentSession` is dropped while pending | `pending_asks` `Arc` drops; oneshot::Sender drops; Receiver gets `RecvError`; tool returns `Err(Cancelled)`. |

### Caller-site changes

| File | Change |
|---|---|
| `cli/src/main.rs` (and other CLI entry points) | `process_direct(...)` → `handle_inbound(...)` |
| `channels/*/src/...` (Telegram, Discord, Slack, …) | All inbound dispatch points switched to `handle_inbound` |
| `engine/src/session/mod.rs` test code, `benches/`, white-box callers | Keep using `process_direct` directly (no pending-ask interference desired) |

`process_direct` is **not deleted** — it becomes `handle_inbound`'s no-pending implementation detail and remains the white-box entry for tests.

## Plugin RPC Surface

```rust
// engine/src/plugin/manifest.rs
pub enum Permission {
    LlmChat, WikiSearch, WikiWrite, WikiDecay,
    SubagentSpawn, MessageSend,
    UserAsk,  // new
}

impl Permission {
    pub fn method_name(&self) -> &'static str {
        match self {
            // …
            Permission::UserAsk => "user/ask",
        }
    }
}

// engine/src/plugin/dispatcher/mod.rs (build_dispatcher)
d.register(Arc::new(ToolDelegateHandler::new(
    "user/ask",
    Permission::UserAsk,
    "ask_user",
))).unwrap();
```

```python
# workspace/plugins/gasket_sdk.py
class GasketPlugin:
    def ask_user(self, prompt: str, timeout_secs: int) -> dict:
        """Ask the user a question and block until answered.

        Returns:
            {"content": str, "sender_id": str, "channel": str,
             "timestamp": str, "media": Optional[dict]}

        Raises:
            RuntimeError: on timeout, already-pending, or session shutdown.
        """
        return self._call("user/ask", {
            "prompt": prompt,
            "timeout_secs": timeout_secs,
        })
```

```yaml
# workspace/plugins/dev_workflow.yaml
permissions:
  - subagent_spawn
  - message_send
  - user_ask           # new
```

## Lifecycle Caveat (slot eviction)

A successfully-registered `PendingAsk` slot must be cleaned up under all paths:

| Path | Cleanup |
|---|---|
| Inbound matches | `try_fulfill` `take`s the slot. |
| Timeout | Tool calls `cancel(key, ask_id)`. |
| Future is aborted (e.g., session shutdown, kernel cancellation) | The `oneshot::Receiver` drops; the next `try_fulfill` finds the `Sender` `is_closed()` and evicts the slot. **Must be implemented in `try_fulfill`.** |
| `AgentSession::drop` | The whole `Arc<PendingAskRegistry>` drops; all senders go away. |

The "Sender closed → evict" branch in `try_fulfill` is the safety net for cancellation. A regression test (`fulfill_after_receiver_dropped_evicts_slot`) freezes this contract.

## Test Strategy

### L1 — `PendingAskRegistry` unit tests

| Test | Invariant |
|---|---|
| `register_then_fulfill` | happy path: register → try_fulfill → Receiver gets `AskAnswer` |
| `register_twice_same_session_rejected` | `Err(AlreadyPending)` |
| `register_two_different_sessions_independent` | distinct keys do not interfere |
| `cancel_clears_slot` | post-cancel, the same key can register again |
| `try_fulfill_no_pending_returns_msg` | miss returns `Err(InboundMessage)` |
| `fulfill_after_receiver_dropped_evicts_slot` | dropped Receiver → next `try_fulfill` evicts; new `register` succeeds |

### L2 — `AskUserTool::execute` integration tests

| Test | Behavior |
|---|---|
| `happy_path` | spawn future → simulate fulfill → JSON answer |
| `timeout_returns_error_and_clears_slot` | `timeout_secs=1`, no fulfill → `Err(Timeout)`, slot empty |
| `cancellation_via_future_drop` | abort future → registry recovers (via L1 eviction path) |
| `outbound_message_sent_with_prompt` | `outbound_tx` receives the prompt before await |
| `missing_registry_in_context_errors_cleanly` | `pending_asks` = None → `ToolError::ExecutionError` (no panic) |

### L3 — `AgentSession::handle_inbound` routing tests

| Test | Scenario |
|---|---|
| `inbound_with_no_pending_calls_process_direct` | mock provider invoked |
| `inbound_with_pending_consumed_short_circuits` | mock provider not invoked; tool future resolves |
| `unrelated_session_inbound_ignored_by_pending` | session A pending, session B inbound → A unaffected, B normal |

### L4 — End-to-end nesting

| Test | Scenario |
|---|---|
| `plugin_subagent_ask_e2e` | plugin → spawn subagent → subagent's LLM calls `ask_user` → simulated user reply → subagent returns → plugin returns |
| `plugin_direct_ask_via_rpc_e2e` | plugin calls `user/ask` directly, simulated reply, plugin returns AskAnswer JSON |
| `concurrent_pending_in_two_sessions` | two sessions ask in parallel, no cross-talk |

### L5 — CLI smoke

Manual: run CLI, main agent calls `ask_user`, prompt prints, user types reply, conversation continues. One smoke script, no heavy automation.

### Definition of Done

- L1–L3 green: `cargo test -p gasket-engine`
- L4 green: `cargo test --workspace --test plugin_ask_user`
- L5 manual: dev_workflow modified version completes one prompt → reply round in CLI mode
- No regression: existing `process_direct` direct-call tests stay green
- Plugin without `user_ask` permission rejected with `Permission denied`

## Linus 5-Layer Analysis

### Layer 1 — Data structure analysis

The core datum is a single `oneshot::Sender<AskAnswer>` keyed by `SessionKey`. Everything else (timeout, prompt, ask_id) is metadata. Ownership model:

- `AgentSession` owns `Arc<PendingAskRegistry>`.
- `Arc<PendingAskRegistry>` owns `Mutex<HashMap<SessionKey, PendingAsk>>`.
- Each `PendingAsk` owns one `oneshot::Sender<AskAnswer>`.
- The corresponding `oneshot::Receiver<AskAnswer>` is held by the awaiting tool future.

No data is copied between owners; the answer flows once via the channel.

### Layer 2 — Edge case identification

The only `if/else` branches in the design are intrinsic, not patches:

- `try_fulfill` hit vs. miss → drives routing decision (intrinsic).
- `register` ok vs. `AlreadyPending` → drives single-slot invariant (intrinsic).
- `tokio::select!` recv vs. timeout → drives caller-controllable lifetime (intrinsic).
- "`Sender::is_closed()` → evict" branch → exists to neutralize future-cancellation; not a feature, a correctness hedge. Documented and tested.

No "if Telegram do X, if CLI do Y" branches. Channel uniformity preserved.

### Layer 3 — Complexity audit

The essence in one sentence: **one slot per session for the next inbound message**. Three concepts: registry, tool, entry-point router. Indentation depth ≤ 2 in all hot paths. The HashMap could in principle be `DashMap` or a per-session `Option<PendingAsk>`, but `Mutex<HashMap>` is the obvious form given current `AgentSession` shape — no premature optimization.

### Layer 4 — Breaking change analysis

| Existing surface | Impact |
|---|---|
| `AgentSession::process_direct` | Unchanged signature/behavior. Still callable directly (kept for tests/benches). |
| `ToolContext` | New `Option` field `pending_asks`. All existing call-sites compile (None default). |
| `EngineHandle` | New `pending_asks` field. Constructed in two places (`AgentSession::process_direct_streaming_with_channel`, RPC dispatcher); both edited in lockstep. |
| `Permission` enum | New variant. Existing manifests without `user_ask` continue to work (default-deny preserved). |
| Plugin RPC method namespace | New `user/ask`. Old plugins unaffected. |
| Channel adapters | Each must switch from `process_direct` to `handle_inbound`. **This is the only required external change.** Adapters that fail to switch will not regress — they simply won't benefit from ask routing. |

No `OutboundMessage` / `InboundMessage` type changes. No broker / types crate changes.

### Layer 5 — Practicality validation

The driving need is concrete (`workspace/plugins/workflows/dev.py`'s research-plan-implement-review loop currently makes assumption-laden inferences when user clarification would be cheaper). Real users hit ambiguity in tasks; the workaround today is to abort and re-prompt manually. The complexity ceiling (single in-memory slot, three new types, one new tool) matches the problem severity. Cross-restart durability and multi-pending routing are explicitly deferred until real demand surfaces.

## Decision Output

```
【核心判断】
值得做：dev_workflow 等工作流插件目前是“猜测式”执行；ask_user 让它们变成“协作式”，
        这是能力跃迁，不是新增 API。

【关键洞察】
- 数据结构：单 oneshot 通道 + HashMap<SessionKey, slot>。所有路由/语义/回收都收
  敛到一处。
- 复杂度：三个新组件、三个改造点；kernel 完全不知情。
- 风险点：future cancellation 与 slot 自动 evict —— 已通过显式 `Sender::is_closed()`
  分支与回归测试 (`fulfill_after_receiver_dropped_evicts_slot`) 锁定。

【Linus 式方案】
1. 第一步是数据结构：`PendingAskRegistry` + `AskAnswer` + `AskError`。
2. 没有特殊情况 —— ChannelType 无关、嵌套 vs. 并发统一为单 slot。
3. 用最笨清晰方式：`Mutex<HashMap>` + `tokio::select!`，不引入 DashMap / 自定义同步。
4. 零破坏：`process_direct` 保留；channels 适配点是一次性切换。
```

## Task List

| # | What | Why | Where | How | Test | Done When |
|---|---|---|---|---|---|---|
| 1 | Add `PendingAskRegistry`, `PendingAsk`, `AskAnswer`, `AskError` types | Foundation data structures | `engine/src/session/pending_ask.rs` (new) | Plain Rust types; `Mutex<HashMap>`; oneshot channels | L1 unit tests (6 listed above) | All L1 tests green |
| 2 | Add `pending_asks: Option<Arc<PendingAskRegistry>>` to `ToolContext` | Tool access path | `engine/src/tools/context.rs` (existing) | Add field with builder method | Existing context tests still compile | `cargo build` green |
| 3 | Implement `AskUserTool` and register in `ToolRegistry` | The new tool itself | `engine/src/tools/ask_user.rs` (new) + tool registry init | `tokio::select!`; emits `OutboundMessage` for prompt | L2 integration tests (5 listed) | All L2 tests green |
| 4 | Add `pending_asks` to `EngineHandle`; thread through `AgentSession::*` | Plugin path inheritance | `engine/src/plugin/dispatcher/mod.rs`; `engine/src/session/mod.rs` | Construct `Arc<PendingAskRegistry>` in `AgentSession::new`; pass into RuntimeContext / EngineHandle | Compilation; existing dispatcher tests green | `cargo test -p gasket-engine` green |
| 5 | Add `AgentSession::handle_inbound` thin entry returning `HandleOutcome` | Inbound routing | `engine/src/session/mod.rs` | Try `try_fulfill`, fall through to `process_direct` | L3 routing tests (3 listed) | All L3 tests green |
| 6 | Add `Permission::UserAsk` and register `user/ask` in dispatcher | Plugin RPC surface | `engine/src/plugin/manifest.rs`; `engine/src/plugin/dispatcher/mod.rs` | One enum variant + one `ToolDelegateHandler::new` line | Existing manifest serde roundtrip; new permission roundtrip | Manifest tests green |
| 7 | Add `ask_user` helper in `gasket_sdk.py` | Plugin SDK | `workspace/plugins/gasket_sdk.py` | One-line `_call("user/ask", ...)` wrapper | L4 e2e tests | All L4 tests green |
| 8 | Switch all inbound entry points to `handle_inbound` | Activate routing | `cli/src/main.rs`; `channels/*/src/*.rs` | Replace `process_direct` calls; map `HandleOutcome::Consumed` to "no reply emitted" | Existing CLI / channel integration tests still pass | All channel tests green |
| 9 | Demonstrate in dev_workflow: clarification phase via `ask_user` | E2E proof | `workspace/plugins/workflows/dev.py`; `workspace/plugins/dev_workflow.yaml` | Add ambiguity-detection subagent + `plugin.ask_user(...)` loop | L5 manual smoke | One round of prompt→reply→continue completes in CLI |

### Acceptance criteria

- All L1–L4 tests green; L5 manual demo recorded in PR.
- Existing `cargo test --workspace` continues to pass.
- A plugin without `user_ask` permission produces `Permission denied` when calling `user/ask`.
- An ask that times out returns a `tool_result` containing the error string (not a panic, not a hang).
- A second `ask_user` call on the same `SessionKey` while the first is pending returns the `already_pending` error string in `tool_result`.
