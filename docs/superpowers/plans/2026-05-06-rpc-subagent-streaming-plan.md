# RPC Subagent Streaming + Dev Workflow Plugin — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the `SubagentSpawnHandler` to forward `StreamEvent`s to the WebSocket in real time, then ship a Python `gasket_sdk` and a `dev_workflow` plugin that proves the fix end-to-end.

**Architecture:** The fix replaces a blocking `spawn` call with `spawn_with_stream` and reuses the existing `spawn_common::spawn_event_forwarder` helper (the same one the native `SpawnTool` uses). On top of the fix, a single-file Python SDK wraps stdio JSON-RPC, and a Research → Plan → Implement → Review workflow plugin demonstrates a 4–8 subagent orchestration with a JSON-verdict review loop.

**Tech Stack:** Rust (tokio, async-trait, serde), Python 3 (stdlib only), JSON-RPC 2.0 over stdio, YAML manifest.

**Spec:** `docs/superpowers/specs/2026-05-05-rpc-subagent-streaming-design.md`

---

## File Structure

| File | Type | Responsibility |
|---|---|---|
| `gasket/engine/src/tools/mod.rs` | modify | Promote `spawn_common` from private to `pub(crate)` |
| `gasket/engine/src/plugin/dispatcher/subagent.rs` | modify | Streaming-aware handler + new test |
| `workspace/plugins/gasket_sdk.py` | new | Single-file Python SDK for JSON-RPC plugins |
| `workspace/plugins/jsonrpc_ping/ping.py` | modify | Migrate to SDK as regression check |
| `workspace/plugins/dev_workflow.yaml` | new | Plugin manifest |
| `workspace/plugins/workflows/dev.py` | new | Workflow logic (research → plan → impl/review loop) |

---

## Task 1: Make `spawn_common` reachable from `plugin::dispatcher::`

**Files:**
- Modify: `gasket/engine/src/tools/mod.rs:38`

This is a one-line visibility change. `spawn_event_forwarder` is already `pub`, but the parent module is private. Doing this in its own commit keeps the streaming refactor commit small and reviewable.

- [ ] **Step 1: Read the current declaration**

Run: `grep -n "spawn_common" gasket/engine/src/tools/mod.rs`

Expected output:
```
38:mod spawn_common;
```

- [ ] **Step 2: Change visibility**

Replace line 38 of `gasket/engine/src/tools/mod.rs`:

```rust
// Before:
mod spawn_common;
// After:
pub(crate) mod spawn_common;
```

- [ ] **Step 3: Verify the crate still builds**

Run: `cargo build -p gasket-engine`
Expected: builds cleanly, no new warnings.

- [ ] **Step 4: Verify nothing else regressed**

Run: `cargo test -p gasket-engine --lib --no-run`
Expected: compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/mod.rs
git commit -m "refactor(engine): expose spawn_common as pub(crate)

Allows plugin::dispatcher::subagent to reuse spawn_event_forwarder
instead of duplicating the StreamEvent → ChatEvent match."
```

---

## Task 2: Refactor `SubagentSpawnHandler` to stream events (TDD)

**Files:**
- Modify: `gasket/engine/src/plugin/dispatcher/subagent.rs` (handler logic + test module)

The current handler swallows all `StreamEvent`s. We'll add a failing streaming test first, then make it pass by switching to `spawn_with_stream` + `spawn_event_forwarder`.

### 2.1 Failing test for streaming behavior

- [ ] **Step 1: Add `MockStreamingSpawner` to the test module**

Append to the `mod tests` block in `gasket/engine/src/plugin/dispatcher/subagent.rs` (after the existing `MockSpawner` definition):

```rust
    use gasket_types::{StreamEvent, StreamEventKind};
    use std::sync::Arc as StdArc;
    use std::sync::Mutex;

    /// Spawner that emits a scripted sequence of StreamEvents through the
    /// streaming channel and then completes the result oneshot.
    struct MockStreamingSpawner {
        scripted_events: StdArc<Mutex<Vec<StreamEvent>>>,
    }

    #[async_trait::async_trait]
    impl SubagentSpawner for MockStreamingSpawner {
        async fn spawn(
            &self,
            _task: String,
            _model_id: Option<String>,
        ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
            unreachable!("streaming handler must call spawn_with_stream, not spawn")
        }

        async fn spawn_with_stream(
            &self,
            task: String,
            model_id: Option<String>,
        ) -> Result<
            (
                String,
                tokio::sync::mpsc::Receiver<StreamEvent>,
                tokio::sync::oneshot::Receiver<SubagentResult>,
                tokio_util::sync::CancellationToken,
            ),
            Box<dyn std::error::Error + Send>,
        > {
            let (event_tx, event_rx) = tokio::sync::mpsc::channel(8);
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let cancel = tokio_util::sync::CancellationToken::new();

            let events: Vec<StreamEvent> =
                self.scripted_events.lock().unwrap().drain(..).collect();
            let task_clone = task.clone();
            let model_clone = model_id.clone();
            tokio::spawn(async move {
                for ev in events {
                    let _ = event_tx.send(ev).await;
                }
                drop(event_tx);
                let _ = result_tx.send(SubagentResult {
                    id: "mock-streaming".to_string(),
                    task: task_clone,
                    response: gasket_types::SubagentResponse {
                        content: "final-content".to_string(),
                        reasoning_content: None,
                        tools_used: vec![],
                        model: None,
                        token_usage: None,
                        cost: 0.0,
                    },
                    model: model_clone,
                });
            });

            Ok(("mock-streaming".to_string(), event_rx, result_rx, cancel))
        }
    }

    fn ctx_with_streaming_spawner(
        scripted: Vec<StreamEvent>,
    ) -> (
        DispatcherContext,
        tokio::sync::mpsc::Receiver<gasket_types::events::OutboundMessage>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let ctx = DispatcherContext {
            engine: Arc::new(EngineHandle {
                session_key: SessionKey::new(
                    gasket_types::events::ChannelType::Telegram,
                    "test-chat",
                ),
                outbound_tx: tx,
                spawner: Arc::new(MockStreamingSpawner {
                    scripted_events: StdArc::new(Mutex::new(scripted)),
                }),
                token_tracker: Arc::new(
                    gasket_types::token_tracker::TokenTracker::unlimited("USD"),
                ),
                tool_registry: Arc::new(ToolRegistry::new()),
                provider: Arc::new(MockProvider),
            }),
        };
        (ctx, rx)
    }
```

- [ ] **Step 2: Add the failing streaming assertion test**

Append the following test to the same `mod tests` block:

```rust
    #[tokio::test]
    async fn test_subagent_spawn_forwards_stream_events() {
        let scripted = vec![
            StreamEvent::thinking("hello-thinking"),
            StreamEvent::content("hello-content"),
        ];
        let (ctx, mut rx) = ctx_with_streaming_spawner(scripted);

        let handler = SubagentSpawnHandler;
        let params = json!({"task": "demo", "model_id": null});
        let result = handler.handle(params, &ctx).await.expect("handler ok");

        // SpawnResponse JSON shape unchanged
        assert_eq!(result["id"], json!("mock-streaming"));
        assert_eq!(result["content"], json!("final-content"));

        // Drain outbound messages with a small budget; collect ChatEvent kinds
        use gasket_types::events::{ChatEvent, OutboundPayload};
        let mut kinds: Vec<&'static str> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await {
                Ok(Some(msg)) => {
                    if let OutboundPayload::WsMessage(ev) = msg.payload {
                        let kind = match ev {
                            ChatEvent::SubagentStarted { .. } => "started",
                            ChatEvent::SubagentThinking { .. } => "thinking",
                            ChatEvent::SubagentContent { .. } => "content",
                            ChatEvent::SubagentToolStart { .. } => "tool_start",
                            ChatEvent::SubagentToolEnd { .. } => "tool_end",
                            _ => continue,
                        };
                        kinds.push(kind);
                    }
                }
                _ => break,
            }
        }
        assert!(kinds.contains(&"started"), "missing SubagentStarted; got {:?}", kinds);
        assert!(kinds.contains(&"thinking"), "missing SubagentThinking; got {:?}", kinds);
        assert!(kinds.contains(&"content"), "missing SubagentContent; got {:?}", kinds);
    }
```

- [ ] **Step 3: Run the test and verify it fails**

Run: `cargo test -p gasket-engine --lib plugin::dispatcher::subagent::tests::test_subagent_spawn_forwards_stream_events -- --nocapture`

Expected: FAIL. Either (a) panic from `unreachable!("streaming handler must call spawn_with_stream...")` because the current handler still calls `spawn`, or (b) assertion failure on missing `started`/`thinking`/`content` events. Either confirms the bug.

If you see "ChatEvent::SubagentStarted does not exist" or similar compile errors, stop and verify the variant names by grepping `gasket/types/src/events/`. Adjust the match arms only — do not change the test's semantic assertions.

### 2.2 Implement the streaming handler

- [ ] **Step 4: Replace the handler body**

Replace the body of `impl RpcHandler for SubagentSpawnHandler::handle` in `gasket/engine/src/plugin/dispatcher/subagent.rs` (currently lines 49-70). The new body:

```rust
    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let spawner = &ctx.engine.spawner;

        let request: SpawnRequest = serde_json::from_value(params).map_err(|e| {
            RpcError::invalid_params(format!("Failed to parse SpawnRequest: {}", e))
        })?;

        // Switch from blocking spawn() to streaming variant so frontend gets
        // live thinking/content events instead of a frozen UI.
        let (subagent_id, event_rx, result_rx, _cancel_token) = spawner
            .spawn_with_stream(request.task.clone(), request.model_id.clone())
            .await
            .map_err(|e| RpcError::internal_error(format!("Subagent spawn failed: {}", e)))?;

        // Notify frontend that the subagent has started (matches SpawnTool behavior).
        let _ = ctx
            .engine
            .outbound_tx
            .send(gasket_types::events::OutboundMessage::with_ws_message(
                ctx.engine.session_key.channel.clone(),
                ctx.engine.session_key.chat_id.clone(),
                gasket_types::events::ChatEvent::subagent_started(
                    subagent_id.clone(),
                    request.task.clone(),
                    0,
                ),
            ))
            .await;

        // Forward StreamEvents → ChatEvents via the shared helper used by SpawnTool.
        let _forward_handle = crate::tools::spawn_common::spawn_event_forwarder(
            subagent_id.clone(),
            event_rx,
            ctx.engine.session_key.clone(),
            ctx.engine.outbound_tx.clone(),
        );

        let result = result_rx.await.map_err(|e| {
            RpcError::internal_error(format!("Subagent result dropped: {}", e))
        })?;

        let response = SpawnResponse {
            id: result.id,
            task: result.task,
            content: result.response.content,
            model: result.model,
        };

        serde_json::to_value(response)
            .map_err(|e| RpcError::internal_error(format!("Failed to serialize response: {}", e)))
    }
```

- [ ] **Step 5: Run the streaming test and verify it passes**

Run: `cargo test -p gasket-engine --lib plugin::dispatcher::subagent::tests::test_subagent_spawn_forwards_stream_events -- --nocapture`

Expected: PASS.

If the test still fails because `subagent_started` / `subagent_thinking` constructor names don't match the actual enum, look at how `gasket/engine/src/tools/spawn.rs:155-160` and `gasket/engine/src/tools/spawn_common.rs:30-46` call them — match those exactly.

- [ ] **Step 6: Run all subagent.rs tests to ensure no regression**

Run: `cargo test -p gasket-engine --lib plugin::dispatcher::subagent::tests`

Expected: all four pre-existing tests (`test_dispatch_success`, `test_dispatch_permission_denied`, `test_dispatch_method_not_found`, `test_dispatch_no_id`) plus the new streaming test all PASS.

- [ ] **Step 7: Run the full engine test suite**

Run: `cargo test -p gasket-engine --lib`

Expected: PASS, no new failures.

- [ ] **Step 8: Commit**

```bash
git add gasket/engine/src/plugin/dispatcher/subagent.rs
git commit -m "fix(plugin): forward StreamEvents from RPC-spawned subagents

SubagentSpawnHandler now uses spawn_with_stream and reuses
spawn_common::spawn_event_forwarder so plugin-driven subagents
produce the same live thinking/content events as the native
SpawnTool. SpawnResponse JSON shape unchanged."
```

---

## Task 3: Python `gasket_sdk`

**Files:**
- Create: `workspace/plugins/gasket_sdk.py`
- Modify: `workspace/plugins/jsonrpc_ping/ping.py` (regression check)

- [ ] **Step 1: Create the SDK file**

Write `workspace/plugins/gasket_sdk.py`:

```python
"""Minimal Gasket plugin SDK for JSON-RPC daemon plugins.

Wraps stdio JSON-RPC 2.0 boilerplate so plugins can focus on logic.
Single-threaded, request-response. No daemon loop — plugin is one-shot;
daemon reuse is handled by the Rust runner.
"""
import json
import sys
from typing import Any, Optional


class GasketPlugin:
    def __init__(self) -> None:
        self._next_id = 1
        self._args: Optional[dict] = None
        self._init_id: Any = None

    # ── lifecycle ──────────────────────────────────────────────────────
    def get_args(self) -> dict:
        """Block until the engine sends the initialize request."""
        if self._args is not None:
            return self._args
        req = self._recv()
        if req is None or req.get("method") != "initialize":
            raise RuntimeError(f"Expected initialize, got: {req}")
        self._init_id = req.get("id")
        self._args = req.get("params", {}) or {}
        return self._args

    def return_result(self, result: dict) -> None:
        """Reply to the initialize request with a successful result."""
        self._send({"jsonrpc": "2.0", "id": self._init_id, "result": result})

    def return_error(self, code: int, message: str) -> None:
        """Reply to the initialize request with an error."""
        self._send({
            "jsonrpc": "2.0",
            "id": self._init_id,
            "error": {"code": code, "message": message},
        })

    # ── engine callbacks ───────────────────────────────────────────────
    def spawn_subagent(self, task: str, model: Optional[str] = None) -> str:
        """Spawn a subagent and block until it returns. Returns content string."""
        params: dict = {"task": task}
        if model is not None:
            params["model_id"] = model
        result = self._call("subagent/spawn", params)
        return result.get("content", "")

    def llm_chat(self, model: str, messages: list, **kwargs: Any) -> dict:
        """Direct LLM chat completion via the engine."""
        return self._call("llm/chat", {"model": model, "messages": messages, **kwargs})

    # ── internals ──────────────────────────────────────────────────────
    def _call(self, method: str, params: dict) -> dict:
        rid = self._next_id
        self._next_id += 1
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        resp = self._recv()
        if resp is None:
            raise RuntimeError(f"stdin closed while waiting for {method}")
        if "error" in resp:
            err = resp["error"]
            raise RuntimeError(
                f"{method} failed: {err.get('message')} (code {err.get('code')})"
            )
        return resp.get("result", {})

    @staticmethod
    def _send(msg: dict) -> None:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

    @staticmethod
    def _recv() -> Optional[dict]:
        line = sys.stdin.readline()
        if not line:
            return None
        return json.loads(line.strip())
```

- [ ] **Step 2: Smoke-test the SDK imports cleanly**

Run: `cd workspace/plugins && python3 -c "from gasket_sdk import GasketPlugin; p = GasketPlugin(); print('ok')"`

Expected output:
```
ok
```

- [ ] **Step 3: Migrate `jsonrpc_ping/ping.py` to use the SDK**

Replace the entire content of `workspace/plugins/jsonrpc_ping/ping.py` with:

```python
#!/usr/bin/env python3
"""JSON-RPC plugin example using gasket_sdk."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from gasket_sdk import GasketPlugin


def main() -> None:
    plugin = GasketPlugin()
    args = plugin.get_args()
    name = args.get("name", "world")

    # Exercise the engine callback path
    chat = plugin.llm_chat(
        model="glm-5",
        messages=[{"role": "user", "content": "hi"}],
    )
    llm_called = bool(chat)

    plugin.return_result({"greeting": f"Hello, {name}!", "llm_called": llm_called})


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run the existing plugin integration test**

Run: `cargo test -p gasket-engine --test '*' jsonrpc_ping 2>/dev/null || cargo test -p gasket-engine --lib jsonrpc 2>/dev/null || cargo test -p gasket-engine plugin 2>&1 | tail -20`

If the project has a dedicated test for `test_ping.yaml`, it should still pass. If no such test exists, run the plugin manually:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"name":"sdk"}}' | \
  python3 workspace/plugins/jsonrpc_ping/ping.py 2>&1 | head -5
```

Expected: at least one outbound JSON line containing `"method":"llm/chat"`. The script will then block waiting for the engine response — kill it with Ctrl+C. The presence of the outbound `llm/chat` line proves the SDK's `_call` works.

- [ ] **Step 5: Commit**

```bash
git add workspace/plugins/gasket_sdk.py workspace/plugins/jsonrpc_ping/ping.py
git commit -m "feat(plugins): add Python gasket_sdk and migrate ping example

SDK wraps JSON-RPC 2.0 stdio boilerplate. Single-threaded
request-response; no daemon loop (Rust runner handles reuse).
ping.py migrated to validate the SDK end-to-end."
```

---

## Task 4: `dev_workflow.yaml` manifest

**Files:**
- Create: `workspace/plugins/dev_workflow.yaml`

- [ ] **Step 1: Create the manifest**

Write `workspace/plugins/dev_workflow.yaml`:

```yaml
name: "dev_workflow"
description: "Research → Plan → Implement → Review loop for code generation"
version: "0.1.0"
protocol: json_rpc
parameters:
  type: object
  properties:
    task:
      type: string
      description: "What to build"
    max_iterations:
      type: integer
      description: "Max review loop iterations (default 3)"
      default: 3
    reasoner_model:
      type: string
      description: "Model for research/plan/review (e.g. 'deepseek/deepseek-reasoner'). Optional; engine default if omitted."
    coder_model:
      type: string
      description: "Model for implementation (e.g. 'openai/gpt-4o'). Optional; engine default if omitted."
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

- [ ] **Step 2: Verify YAML parses by running the manifest loader**

Run:

```bash
python3 -c "import yaml; print(yaml.safe_load(open('workspace/plugins/dev_workflow.yaml')))" 2>&1 | head -3
```

Expected: a single line of dict output starting with `{'name': 'dev_workflow', ...}`. No `yaml.YAMLError`.

- [ ] **Step 3: Sanity-check manifest matches the engine's `PluginManifest` schema**

Run: `grep -n "name:\|protocol:\|permissions:\|timeout_secs:" gasket/engine/src/plugin/manifest.rs | head -10`

Expected: confirm `name`, `protocol`, `runtime.timeout_secs`, `permissions` are all valid field names. The `parameters` block uses JSON Schema and is passed through.

- [ ] **Step 4: Commit**

```bash
git add workspace/plugins/dev_workflow.yaml
git commit -m "feat(plugins): add dev_workflow manifest

Declares JSON-RPC plugin requesting subagent_spawn permission.
Per-node model selection via reasoner_model/coder_model inputs.
timeout_secs 1200s upper bound for full workflow."
```

---

## Task 5: `dev_workflow` Python logic

**Files:**
- Create: `workspace/plugins/workflows/dev.py`

- [ ] **Step 1: Create the workflow directory**

Run: `mkdir -p workspace/plugins/workflows`

- [ ] **Step 2: Write the workflow script**

Write `workspace/plugins/workflows/dev.py`:

```python
#!/usr/bin/env python3
"""Dev workflow: Research → Plan → Implement → Review loop.

Orchestrates 4–8 subagents via subagent/spawn. Implements bounded retry
with strict-JSON verdict parsing; appends reviewer feedback to the plan
on failure rather than replacing it (preserves original goals).
"""
import json
import sys
from pathlib import Path
from typing import Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from gasket_sdk import GasketPlugin


REVIEW_PROMPT_SUFFIX = """

Output STRICT JSON only, no prose, no markdown fences:
{"verdict": "PASS" | "FAIL", "reason": "<one sentence>"}
"""


def parse_verdict(review_text: str) -> Tuple[str, str]:
    """Parse strict JSON verdict; tolerate ``` fences as fallback.

    Returns (verdict, reason) where verdict is exactly "PASS" or "FAIL".
    Unparseable input is treated as FAIL.
    """
    txt = review_text.strip()
    if txt.startswith("```"):
        # Strip ```json ... ``` fences if model misbehaves
        txt = txt.strip("`")
        if txt.lower().startswith("json"):
            txt = txt[4:]
        txt = txt.strip()
    try:
        obj = json.loads(txt)
    except json.JSONDecodeError:
        return "FAIL", f"reviewer output not parseable: {review_text[:200]}"
    verdict = str(obj.get("verdict", "FAIL")).upper()
    reason = str(obj.get("reason", ""))
    if verdict not in ("PASS", "FAIL"):
        verdict = "FAIL"
    return verdict, reason


def main() -> None:
    plugin = GasketPlugin()
    args = plugin.get_args()

    task = args["task"]
    max_iter = int(args.get("max_iterations", 3))
    reasoner = args.get("reasoner_model")  # None → engine default
    coder = args.get("coder_model")

    # Phase 1: Research
    research = plugin.spawn_subagent(
        f"Research strictly relevant context for this task. Be concise.\n\n"
        f"Task: {task}",
        model=reasoner,
    )

    # Phase 2: Plan
    plan = plugin.spawn_subagent(
        f"Create concrete implementation steps based on research.\n\n"
        f"Task: {task}\n\nResearch:\n{research}",
        model=reasoner,
    )

    # Phase 3: Implement → Review loop (best-effort)
    code = ""
    last_reason = ""
    passed = False
    iterations_used = 0
    for i in range(max_iter):
        iterations_used = i + 1
        code = plugin.spawn_subagent(
            f"Implement this plan. Output runnable code only.\n\n"
            f"Plan:\n{plan}\n\nPrevious attempt (may be empty):\n{code}",
            model=coder,
        )
        review = plugin.spawn_subagent(
            f"Review this code against the plan. Be strict.{REVIEW_PROMPT_SUFFIX}\n\n"
            f"Plan:\n{plan}\n\nCode:\n{code}",
            model=reasoner,
        )
        verdict, reason = parse_verdict(review)
        last_reason = reason
        if verdict == "PASS":
            passed = True
            break
        # Append reviewer feedback (do not replace plan — preserves original goals)
        plan = f"{plan}\n\n[Reviewer feedback to address]:\n{reason}"

    plugin.return_result({
        "final_code": code,
        "passed": passed,
        "iterations_used": iterations_used,
        "last_review_reason": last_reason,
    })


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Smoke-test parsing helper in isolation**

Run:

```bash
python3 -c "
import sys
sys.path.insert(0, 'workspace/plugins/workflows')
from dev import parse_verdict
assert parse_verdict('{\"verdict\":\"PASS\",\"reason\":\"ok\"}') == ('PASS', 'ok')
assert parse_verdict('\`\`\`json\n{\"verdict\":\"FAIL\",\"reason\":\"x\"}\n\`\`\`') == ('FAIL', 'x')
assert parse_verdict('not json') == ('FAIL', 'reviewer output not parseable: not json')
assert parse_verdict('{\"verdict\":\"MAYBE\",\"reason\":\"\"}') == ('FAIL', '')
print('ok')
"
```

Expected output:
```
ok
```

If any assertion fails, fix `parse_verdict` until all four pass before continuing.

- [ ] **Step 4: Syntax-check the full script**

Run: `python3 -m py_compile workspace/plugins/workflows/dev.py && echo "syntax ok"`

Expected:
```
syntax ok
```

- [ ] **Step 5: Commit**

```bash
git add workspace/plugins/workflows/dev.py
git commit -m "feat(plugins): add dev_workflow Research→Plan→Implement→Review

Python script orchestrates 4–8 subagents via subagent/spawn.
Strict JSON verdict parsing with fence tolerance; appends reviewer
feedback to plan on FAIL (preserves original goals).
Returns best-effort result with passed/iterations_used metadata."
```

---

## Task 6: End-to-end verification

**Files:** none (manual verification)

This task validates that all four pieces compose correctly. No new code; only execution and observation.

- [ ] **Step 1: Build the CLI**

Run: `cargo build -p gasket-cli`
Expected: builds cleanly.

- [ ] **Step 2: Execute the workflow with a tiny task**

Run:

```bash
cargo run -p gasket-cli -- tool execute dev_workflow '{"task":"write a one-line hello world in python","max_iterations":2}' 2>&1 | tail -40
```

Expected: command exits 0 within `timeout_secs` (1200s, but a small task should finish in 1–3 minutes). The final stdout JSON contains keys `final_code`, `passed`, `iterations_used`, `last_review_reason`.

If `tool execute dev_workflow` is rejected with "tool not found", verify the plugin discovery path. Run `cargo run -p gasket-cli -- tool list 2>&1 | grep dev_workflow` — if missing, check that the manifest is in the directory the engine scans (look at how `test_ping` is discovered).

- [ ] **Step 3: Verify streaming events appear in logs**

Run with debug logging:

```bash
RUST_LOG=gasket_engine::plugin=debug,gasket_engine::tools::spawn_common=debug cargo run -p gasket-cli -- tool execute dev_workflow '{"task":"write a one-line hello world in python","max_iterations":2}' 2>&1 | grep -E "subagent_started|spawn_event_forwarder|StreamEvent" | head -20
```

Expected: at least 4 distinct subagent IDs appear in log lines (research, plan, implement#1, review#1). Each subagent produces multiple `Thinking` / `Content` events. If you see zero forwarded events but the task completes, Task 2 didn't take — re-check the handler diff.

- [ ] **Step 4: Manually inspect the JSON output structure**

Run:

```bash
cargo run -p gasket-cli -- tool execute dev_workflow '{"task":"write a one-line hello world in python","max_iterations":1}' 2>/dev/null | tail -1 | python3 -c "import json,sys; obj = json.loads(sys.stdin.read()); print(sorted(obj.keys()))"
```

Expected output (key order may vary):
```
['final_code', 'iterations_used', 'last_review_reason', 'passed']
```

- [ ] **Step 5: Final commit (if any docs need updating)**

If you discovered any spec inaccuracy during E2E, update the spec inline and:

```bash
git add docs/superpowers/specs/2026-05-05-rpc-subagent-streaming-design.md
git commit -m "docs(spec): post-implementation corrections to dev_workflow design"
```

If no corrections, skip this step — Task 6 is verification-only and produces no commit by itself.

---

## Summary

| Task | What | Lines | Risk |
|---|---|---|---|
| 1 | `pub(crate) mod spawn_common` | 1 | Trivial |
| 2 | Streaming handler + test | ~120 (mostly test) | Mock setup verbose; test asserts ChatEvent variants — name match required |
| 3 | Python SDK + ping migration | ~90 | None; pure stdlib |
| 4 | YAML manifest | ~25 | Field name match with `manifest.rs` |
| 5 | Workflow Python | ~100 | Real LLM cost during E2E (Task 6) |
| 6 | E2E verification | 0 | Network + LLM credits required |

**Total engine code change: ~30 lines** (1 line visibility + handler body replacement). Everything else is plugin code outside the engine crate.
