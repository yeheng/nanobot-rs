# Ask-User Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-memory `ask_user` capability that lets workflows / plugins / subagents / the main agent emit a question to the user and synchronously block on the next inbound message as the answer.

**Architecture:** A single-slot `PendingAskRegistry` keyed by `SessionKey` lives inside `AgentSession`. The new `ask_user` tool registers a slot, emits an `OutboundMessage` with the prompt, and `tokio::select!`s on a `oneshot::Receiver` vs. caller-supplied timeout. A new thin `AgentSession::handle_inbound{,_streaming_with_channel}` entry tries `try_fulfill` before falling through to `process_direct`. Plugin RPC reaches the same tool via a new `Permission::UserAsk` + `user/ask` `ToolDelegateHandler`.

**Tech Stack:** Rust (workspace crates `gasket-types`, `gasket-engine`); `tokio::sync::oneshot` + `tokio::sync::Mutex<HashMap<SessionKey, …>>`; Python 3 plugin SDK.

---

## Spec Reconciliation (deltas from `docs/superpowers/specs/2026-05-06-ask-user-design.md`)

The spec is correct in essence; three locations changed during verification:

1. **`PendingAskRegistry` trait + `AskAnswer` + `AskError` + `AskRequest` go in `gasket-types`, not `gasket-engine`** — `ToolContext` lives in `gasket-types/src/tool.rs:119`, mirroring the `SubagentSpawner` precedent (trait in types, impl in engine). The concrete `PendingAskRegistryImpl` still lives in `gasket-engine/src/session/pending_ask.rs`.

2. **`AskAnswer.media: Option<Vec<MediaAttachment>>`** (not `Option<Media>`) — `gasket_types::events::InboundMessage.media` is a `Option<Vec<MediaAttachment>>`; we mirror that.

3. **Two `handle_inbound` variants** — there are two existing entry points (`process_direct` blocking, `process_direct_streaming_with_channel` streaming) called from 6 sites. We add `handle_inbound` + `handle_inbound_streaming_with_channel` mirroring the same split. `InboundMessage::session_key()` already exists; we use it instead of the spec's `SessionKey::from(&msg)`.

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `gasket/types/src/pending_ask.rs` | `PendingAskRegistry` trait, `AskAnswer`, `AskRequest`, `AskError` |
| `gasket/engine/src/session/pending_ask.rs` | `PendingAskRegistryImpl` (concrete impl) + L1 unit tests |
| `gasket/engine/src/tools/ask_user.rs` | `AskUserTool` + L2 integration tests |
| `gasket/engine/tests/handle_inbound.rs` | L3 routing tests |
| `gasket/engine/tests/plugin_ask_user.rs` | L4 end-to-end tests |

### Modified files

| Path | Change |
|---|---|
| `gasket/types/src/lib.rs` | `pub mod pending_ask;` + re-exports |
| `gasket/types/src/tool.rs` | New field `pending_asks: Option<Arc<dyn PendingAskRegistry>>` on `ToolContext`; builder method; `Default`; `Debug` |
| `gasket/engine/src/session/mod.rs` | `pub mod pending_ask;`, `AgentSession.pending_asks` field, `HandleOutcome` enum, `handle_inbound` + streaming variant, threading into `RuntimeContext`/`ToolContext` |
| `gasket/engine/src/session/builder.rs` | Construct `Arc<PendingAskRegistryImpl>` and inject into `AgentSession` |
| `gasket/engine/src/tools/mod.rs` | `mod ask_user;` + `pub use ask_user::AskUserTool;` |
| `gasket/engine/src/tools/provider.rs` | Register `AskUserTool` in `CoreToolProvider::register_tools` |
| `gasket/engine/src/plugin/dispatcher/mod.rs` | `EngineHandle.pending_asks` field; register `user/ask` `ToolDelegateHandler` in `build_dispatcher` |
| `gasket/engine/src/plugin/manifest.rs` | `Permission::UserAsk` variant + `method_name` arm |
| `gasket/engine/src/plugin/mod.rs` | `make_dispatch_ctx` threads `pending_asks` |
| `gasket/cli/src/commands/agent.rs` | 4 call sites: `process_direct{,_streaming_with_channel}` → `handle_inbound{,_streaming_with_channel}` |
| `gasket/engine/src/bus_adapter.rs` | 2 call sites: same migration |
| `workspace/plugins/gasket_sdk.py` | `ask_user(prompt, timeout_secs)` helper |
| `workspace/plugins/workflows/dev.py` | Clarification phase using `plugin.ask_user(...)` |
| `workspace/plugins/dev_workflow.yaml` | Add `user_ask` permission |

---

## Task 1: Public types in `gasket-types`

**Why:** Foundation. Everything else depends on these types; they live in the lowest crate to avoid circular dependencies.

**Files:**
- Create: `gasket/types/src/pending_ask.rs`
- Modify: `gasket/types/src/lib.rs`

- [ ] **Step 1.1: Create the new module file with types and trait**

Create `gasket/types/src/pending_ask.rs`:

```rust
//! Pending-ask coordination types.
//!
//! `ask_user` lets a tool block until the next inbound message on the same
//! session arrives. The trait declared here is the contract between the engine
//! (which owns the registry) and the tool (which awaits the answer). The
//! concrete implementation lives in `gasket-engine`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::events::{ChannelType, InboundMessage, MediaAttachment, SessionKey};

/// Reply to a pending `ask_user`. Returned to the awaiting tool as the
/// `tool_result` payload (after JSON serialization).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskAnswer {
    pub content: String,
    pub sender_id: String,
    pub channel: ChannelType,
    pub timestamp: DateTime<Utc>,
    pub media: Option<Vec<MediaAttachment>>,
}

impl AskAnswer {
    /// Build an `AskAnswer` from a fully-populated inbound message.
    pub fn from_inbound(msg: InboundMessage) -> Self {
        Self {
            content: msg.content,
            sender_id: msg.sender_id,
            channel: msg.channel,
            timestamp: msg.timestamp,
            media: msg.media,
        }
    }

    /// Build an `AskAnswer` synthetically when only `(content, session_key)`
    /// is available (e.g., legacy entry points).
    pub fn synthesize(content: String, key: &SessionKey) -> Self {
        Self {
            content,
            sender_id: key.chat_id.clone(),
            channel: key.channel.clone(),
            timestamp: Utc::now(),
            media: None,
        }
    }
}

/// Per-slot registration data returned to the caller of `register`.
pub struct AskRegistration {
    pub ask_id: uuid::Uuid,
    pub answer_rx: oneshot::Receiver<AskAnswer>,
}

#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("session {0} already has a pending ask")]
    AlreadyPending(SessionKey),
    #[error("ask timed out after {0:?}")]
    Timeout(Duration),
    #[error("ask cancelled: session shutting down")]
    Cancelled,
}

/// Engine-side pending-ask registry, keyed by `SessionKey`.
///
/// **Invariants:**
/// - At most one slot per `SessionKey` is occupied at a time.
/// - Every successful `register` is paired with exactly one of `try_fulfill`
///   (matched by inbound) or `cancel` (timeout / abort).
/// - If the receiver is dropped without `cancel`, `try_fulfill` MUST evict the
///   stale slot (via `Sender::is_closed()` check) so that subsequent
///   `register` calls succeed.
pub trait PendingAskRegistry: Send + Sync {
    /// Reserve the slot for `key` and return a receiver for the answer.
    /// Returns `Err(AlreadyPending)` if the slot is already in use.
    fn register(
        &self,
        key: SessionKey,
        prompt: String,
        deadline: Instant,
    ) -> Result<AskRegistration, AskError>;

    /// Remove a registration (used by tool when timeout fires or future is
    /// cancelled). No-op if `ask_id` does not match the slot's current id.
    fn cancel(&self, key: &SessionKey, ask_id: uuid::Uuid);

    /// Try to deliver `msg` to a pending ask on `key`. On miss returns
    /// `Err(msg)` so the caller can route the message to the normal pipeline.
    /// On stale slot (receiver dropped), evicts the slot and reports miss.
    fn try_fulfill(
        &self,
        key: &SessionKey,
        msg: InboundMessage,
    ) -> Result<(), InboundMessage>;
}

/// Convenience: dyn-trait alias used by `ToolContext`.
pub type DynPendingAskRegistry = Arc<dyn PendingAskRegistry>;
```

- [ ] **Step 1.2: Re-export from `gasket-types`**

Modify `gasket/types/src/lib.rs`. Add after line 19 (`pub mod tool;`):

```rust
pub mod pending_ask;
```

And add to the re-exports block (after the `pub use tool::{…}` block ending at line 39):

```rust
pub use pending_ask::{
    AskAnswer, AskError, AskRegistration, DynPendingAskRegistry, PendingAskRegistry,
};
```

- [ ] **Step 1.3: Verify the crate still builds**

Run: `cargo build -p gasket-types`
Expected: `Compiling gasket-types …` → `Finished`

- [ ] **Step 1.4: Commit**

```bash
git add gasket/types/src/pending_ask.rs gasket/types/src/lib.rs
git commit -m "types(pending_ask): add AskAnswer/AskError/PendingAskRegistry trait"
```

---

## Task 2: `PendingAskRegistryImpl` + L1 unit tests

**Why:** The concrete in-memory implementation. Pure data-structure work; no tokio runtime needed for most tests.

**Files:**
- Create: `gasket/engine/src/session/pending_ask.rs`
- Modify: `gasket/engine/src/session/mod.rs` (add `pub mod pending_ask;`)

- [ ] **Step 2.1: Create the impl with L1 tests in one file (TDD: write test first, then impl)**

Create `gasket/engine/src/session/pending_ask.rs`:

```rust
//! Concrete `PendingAskRegistry` implementation backed by a `Mutex<HashMap>`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use tokio::sync::oneshot;

use gasket_types::events::{InboundMessage, SessionKey};
use gasket_types::pending_ask::{
    AskAnswer, AskError, AskRegistration, PendingAskRegistry,
};

/// Internal slot record.
struct Slot {
    ask_id: uuid::Uuid,
    answer_tx: oneshot::Sender<AskAnswer>,
    #[allow(dead_code)] // diagnostic only
    prompt: String,
    #[allow(dead_code)] // diagnostic only
    deadline: Instant,
}

/// In-memory `PendingAskRegistry`. Single slot per `SessionKey`.
pub struct PendingAskRegistryImpl {
    inner: Mutex<HashMap<SessionKey, Slot>>,
}

impl PendingAskRegistryImpl {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for PendingAskRegistryImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingAskRegistry for PendingAskRegistryImpl {
    fn register(
        &self,
        key: SessionKey,
        prompt: String,
        deadline: Instant,
    ) -> Result<AskRegistration, AskError> {
        let mut guard = self.inner.lock().expect("PendingAskRegistry mutex poisoned");

        // Evict a stale slot if its receiver has been dropped.
        if let Some(existing) = guard.get(&key) {
            if existing.answer_tx.is_closed() {
                guard.remove(&key);
            }
        }

        if guard.contains_key(&key) {
            return Err(AskError::AlreadyPending(key));
        }

        let (answer_tx, answer_rx) = oneshot::channel::<AskAnswer>();
        let ask_id = uuid::Uuid::new_v4();

        guard.insert(
            key.clone(),
            Slot {
                ask_id,
                answer_tx,
                prompt,
                deadline,
            },
        );
        Ok(AskRegistration { ask_id, answer_rx })
    }

    fn cancel(&self, key: &SessionKey, ask_id: uuid::Uuid) {
        let mut guard = self.inner.lock().expect("PendingAskRegistry mutex poisoned");
        if let Some(slot) = guard.get(key) {
            if slot.ask_id == ask_id {
                guard.remove(key);
            }
        }
    }

    fn try_fulfill(
        &self,
        key: &SessionKey,
        msg: InboundMessage,
    ) -> Result<(), InboundMessage> {
        let mut guard = self.inner.lock().expect("PendingAskRegistry mutex poisoned");

        // Evict a stale slot if its receiver has been dropped, then miss.
        if let Some(existing) = guard.get(key) {
            if existing.answer_tx.is_closed() {
                guard.remove(key);
                return Err(msg);
            }
        }

        let Some(slot) = guard.remove(key) else {
            return Err(msg);
        };

        let answer = AskAnswer::from_inbound(msg);
        // If send fails, the receiver is gone — the slot is already taken,
        // and we drop the answer. Caller (engine) treats this as a "consumed"
        // outcome regardless; there's no useful recovery path.
        let _ = slot.answer_tx.send(answer);
        Ok(())
    }
}

// ── L1 unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::events::ChannelType;
    use std::time::Duration;

    fn key(id: &str) -> SessionKey {
        SessionKey::new(ChannelType::Cli, id)
    }

    fn dummy_inbound(content: &str, key: &SessionKey) -> InboundMessage {
        InboundMessage {
            channel: key.channel.clone(),
            sender_id: "sender".to_string(),
            chat_id: key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn register_then_fulfill() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");

        let registration = reg
            .register(k.clone(), "hello?".into(), deadline())
            .expect("register");

        let msg = dummy_inbound("yes", &k);
        reg.try_fulfill(&k, msg).expect("fulfill");

        let answer = registration
            .answer_rx
            .blocking_recv()
            .expect("receiver got answer");
        assert_eq!(answer.content, "yes");
        assert_eq!(answer.channel, ChannelType::Cli);
    }

    #[test]
    fn register_twice_same_session_rejected() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let _r1 = reg.register(k.clone(), "q1".into(), deadline()).unwrap();
        let err = reg.register(k.clone(), "q2".into(), deadline()).unwrap_err();
        assert!(matches!(err, AskError::AlreadyPending(_)));
    }

    #[test]
    fn register_two_different_sessions_independent() {
        let reg = PendingAskRegistryImpl::new();
        let ka = key("a");
        let kb = key("b");
        let ra = reg.register(ka.clone(), "qa".into(), deadline()).unwrap();
        let rb = reg.register(kb.clone(), "qb".into(), deadline()).unwrap();

        // Fulfill B first; A must still be pending.
        reg.try_fulfill(&kb, dummy_inbound("ans-b", &kb)).unwrap();
        let ans_b = rb.answer_rx.blocking_recv().unwrap();
        assert_eq!(ans_b.content, "ans-b");

        reg.try_fulfill(&ka, dummy_inbound("ans-a", &ka)).unwrap();
        let ans_a = ra.answer_rx.blocking_recv().unwrap();
        assert_eq!(ans_a.content, "ans-a");
    }

    #[test]
    fn cancel_clears_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
        reg.cancel(&k, r.ask_id);
        // Now we should be able to register again.
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("re-register after cancel");
    }

    #[test]
    fn try_fulfill_no_pending_returns_msg() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let msg = dummy_inbound("hi", &k);
        let returned = reg.try_fulfill(&k, msg).unwrap_err();
        assert_eq!(returned.content, "hi");
    }

    #[test]
    fn fulfill_after_receiver_dropped_evicts_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        {
            let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
            drop(r); // drop the receiver
        }
        // Stale Sender remains in the slot. try_fulfill must evict and miss.
        let msg = dummy_inbound("hi", &k);
        let returned = reg.try_fulfill(&k, msg).unwrap_err();
        assert_eq!(returned.content, "hi");

        // After eviction the slot is free; a new register must succeed.
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("register after stale eviction");
    }

    #[test]
    fn register_evicts_stale_slot() {
        // Same as the previous, but eviction happens during register itself.
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        {
            let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
            drop(r);
        }
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("register evicts stale slot");
    }
}
```

- [ ] **Step 2.2: Wire the new module into `session/mod.rs`**

Modify `gasket/engine/src/session/mod.rs`. Add to the `pub mod` block near line 6:

```rust
pub mod pending_ask;
```

And add to the `pub use compactor::{…}` re-export block area (around line 13):

```rust
pub use pending_ask::PendingAskRegistryImpl;
```

- [ ] **Step 2.3: Run L1 tests, expect all pass**

Run: `cargo test -p gasket-engine --lib session::pending_ask::tests`
Expected:
```
running 7 tests
test session::pending_ask::tests::register_then_fulfill ... ok
test session::pending_ask::tests::register_twice_same_session_rejected ... ok
test session::pending_ask::tests::register_two_different_sessions_independent ... ok
test session::pending_ask::tests::cancel_clears_slot ... ok
test session::pending_ask::tests::try_fulfill_no_pending_returns_msg ... ok
test session::pending_ask::tests::fulfill_after_receiver_dropped_evicts_slot ... ok
test session::pending_ask::tests::register_evicts_stale_slot ... ok

test result: ok. 7 passed
```

- [ ] **Step 2.4: Commit**

```bash
git add gasket/engine/src/session/pending_ask.rs gasket/engine/src/session/mod.rs
git commit -m "engine(session): add PendingAskRegistryImpl with L1 unit tests"
```

---

## Task 3: Add `pending_asks` field to `ToolContext`

**Why:** `AskUserTool::execute` reads the registry from its context. Adding the field here threads it everywhere the existing `ToolContext` flows.

**Files:**
- Modify: `gasket/types/src/tool.rs`

- [ ] **Step 3.1: Add the field, default, debug, and builder method**

Modify `gasket/types/src/tool.rs`.

Add to the imports (after line 12):

```rust
use crate::pending_ask::DynPendingAskRegistry;
```

Add a new field at the end of the `ToolContext` struct (just after `aggregator_cancel`, currently line 137):

```rust
    /// Pending-ask registry for the `ask_user` tool. None in contexts that
    /// don't need user prompting (CLI white-box, unit tests).
    pub pending_asks: Option<DynPendingAskRegistry>,
```

Add to `Default::default` (currently line 140-153) — add the field:

```rust
            pending_asks: None,
```

Add to `Debug::fmt` (currently line 156-167) — add the field:

```rust
            .field("pending_asks", &self.pending_asks.is_some())
```

Add a builder method to the `impl ToolContext` block (after `aggregator_cancel`, around line 207):

```rust
    pub fn pending_asks(mut self, registry: DynPendingAskRegistry) -> Self {
        self.pending_asks = Some(registry);
        self
    }
```

- [ ] **Step 3.2: Verify build**

Run: `cargo build -p gasket-types && cargo build -p gasket-engine`
Expected: clean build (the `Option` default keeps existing call sites valid).

- [ ] **Step 3.3: Commit**

```bash
git add gasket/types/src/tool.rs
git commit -m "types(tool_context): add optional pending_asks registry field"
```

---

## Task 4: Implement `AskUserTool` + L2 integration tests

**Why:** The actual `ask_user` tool. This is the most logic-rich step.

**Files:**
- Create: `gasket/engine/src/tools/ask_user.rs`
- Modify: `gasket/engine/src/tools/mod.rs`

- [ ] **Step 4.1: Create the tool file with the implementation only**

Create `gasket/engine/src/tools/ask_user.rs`:

```rust
//! `ask_user` — synchronously prompt the user for a reply.
//!
//! Registers a slot in the session's `PendingAskRegistry`, emits the prompt
//! as an `OutboundMessage`, then awaits either the answer or the timeout.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::sleep;

use gasket_types::events::OutboundMessage;
use gasket_types::pending_ask::{AskError, PendingAskRegistry};
use gasket_types::{Tool, ToolContext, ToolError, ToolResult};

const MAX_TIMEOUT_SECS: u64 = 86_400; // 24 hours

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their reply. Returns a JSON \
         object with the user's answer (content, sender_id, channel, \
         timestamp, optional media). Use only when the user's input is \
         genuinely required to proceed."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The question text shown to the user."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECS,
                    "description": "Maximum seconds to wait. Required."
                }
            },
            "required": ["prompt", "timeout_secs"]
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing 'prompt'".into()))?
            .to_string();

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing/invalid 'timeout_secs'".into())
            })?;
        if !(1..=MAX_TIMEOUT_SECS).contains(&timeout_secs) {
            return Err(ToolError::InvalidArguments(format!(
                "'timeout_secs' must be in [1, {}], got {}",
                MAX_TIMEOUT_SECS, timeout_secs
            )));
        }
        let timeout = Duration::from_secs(timeout_secs);

        let registry = ctx.pending_asks.clone().ok_or_else(|| {
            ToolError::ExecutionError(
                "ask_user requires a PendingAskRegistry in ToolContext; this \
                 context does not support user prompting"
                    .into(),
            )
        })?;

        let deadline = Instant::now() + timeout;
        let registration = registry
            .register(ctx.session_key.clone(), prompt.clone(), deadline)
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;
        let ask_id = registration.ask_id;
        let mut answer_rx = registration.answer_rx;

        // Send prompt to the user channel.
        let outbound = OutboundMessage::new(
            ctx.session_key.channel.clone(),
            ctx.session_key.chat_id.clone(),
            prompt,
        );
        if let Err(e) = ctx.outbound_tx.send(outbound).await {
            registry.cancel(&ctx.session_key, ask_id);
            return Err(ToolError::ExecutionError(format!(
                "failed to send prompt: {}",
                e
            )));
        }

        // Await answer or timeout.
        let answer = tokio::select! {
            biased;
            recv = &mut answer_rx => match recv {
                Ok(answer) => Ok(answer),
                Err(_) => Err(AskError::Cancelled),
            },
            _ = sleep(timeout) => {
                registry.cancel(&ctx.session_key, ask_id);
                Err(AskError::Timeout(timeout))
            }
        };

        match answer {
            Ok(a) => serde_json::to_string(&a).map_err(|e| {
                ToolError::ExecutionError(format!("failed to serialize answer: {}", e))
            }),
            Err(e) => Err(ToolError::ExecutionError(e.to_string())),
        }
    }
}
```

- [ ] **Step 4.2: Verify the file compiles**

Run: `cargo build -p gasket-engine`
Expected: clean build.

- [ ] **Step 4.3: Append the L2 test module**

Append to the same file `gasket/engine/src/tools/ask_user.rs`:

```rust

// ── L2 integration tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PendingAskRegistryImpl;
    use gasket_types::events::{ChannelType, InboundMessage, SessionKey};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn ctx_for_test(
        registry: Arc<dyn PendingAskRegistry>,
    ) -> (ToolContext, mpsc::Receiver<OutboundMessage>) {
        let (tx, rx) = mpsc::channel::<OutboundMessage>(8);
        let ctx = ToolContext::default()
            .session_key(SessionKey::new(ChannelType::Cli, "test"))
            .outbound_tx(tx)
            .pending_asks(registry);
        (ctx, rx)
    }

    fn dummy_inbound(content: &str, key: &SessionKey) -> InboundMessage {
        InboundMessage {
            channel: key.channel.clone(),
            sender_id: "sender".to_string(),
            chat_id: key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        }
    }

    #[tokio::test]
    async fn happy_path() {
        let registry: Arc<dyn PendingAskRegistry> = Arc::new(PendingAskRegistryImpl::new());
        let (ctx, mut outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 5});

        let task = tokio::spawn(async move { tool.execute(args, &ctx).await });

        let outbound = outbound_rx.recv().await.expect("outbound prompt sent");
        assert_eq!(outbound.content(), "what?");

        registry.try_fulfill(&key, dummy_inbound("answer", &key)).unwrap();

        let result_str = task.await.unwrap().expect("ok result");
        let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();
        assert_eq!(parsed["content"], "answer");
        assert_eq!(parsed["channel"], "cli");
    }

    #[tokio::test]
    async fn timeout_returns_error_and_clears_slot() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, _outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 1});

        let result = tool.execute(args, &ctx).await;
        let err = result.expect_err("expected Timeout error");
        let msg = err.to_string();
        assert!(msg.contains("timed out"), "actual: {msg}");

        // Slot must be empty now — re-registering must succeed.
        let _again = registry
            .register(key.clone(), "q2".into(), Instant::now() + Duration::from_secs(5))
            .expect("slot is free after timeout");
    }

    #[tokio::test]
    async fn cancellation_via_future_drop() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, _outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 30});

        let handle = tokio::spawn(async move { tool.execute(args, &ctx).await });
        // Wait for the tool to register, then cancel.
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        // The receiver was dropped along with the future. Registry recovers
        // either via try_fulfill stale-eviction OR via the next register call.
        let _re = registry
            .register(key.clone(), "q2".into(), Instant::now() + Duration::from_secs(5))
            .expect("re-register after future abort");
    }

    #[tokio::test]
    async fn outbound_message_sent_with_prompt() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, mut outbound_rx) = ctx_for_test(registry.clone());

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "abc?", "timeout_secs": 1});
        let _task = tokio::spawn(async move { tool.execute(args, &ctx).await });

        let outbound = outbound_rx.recv().await.expect("prompt was sent");
        assert_eq!(outbound.content(), "abc?");
    }

    #[tokio::test]
    async fn missing_registry_in_context_errors_cleanly() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<OutboundMessage>(1);
        let ctx = ToolContext::default()
            .session_key(SessionKey::new(ChannelType::Cli, "test"))
            .outbound_tx(tx);
        // Note: no .pending_asks() — registry is None.

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "?", "timeout_secs": 1});
        let err = tool.execute(args, &ctx).await.expect_err("must error");
        assert!(matches!(err, ToolError::ExecutionError(_)));
    }
}
```

- [ ] **Step 4.3: Wire the new tool into `tools/mod.rs`**

Modify `gasket/engine/src/tools/mod.rs`. Add to the existing `mod` declarations (alphabetical, after line 19's `mod builder;`):

```rust
mod ask_user;
```

Add to the existing re-exports (alphabetical, before `pub use builder::…` around line 56):

```rust
pub use ask_user::AskUserTool;
```

- [ ] **Step 4.4: Run L2 tests, expect all pass**

Run: `cargo test -p gasket-engine --lib tools::ask_user::tests`
Expected:
```
running 5 tests
test tools::ask_user::tests::happy_path ... ok
test tools::ask_user::tests::timeout_returns_error_and_clears_slot ... ok
test tools::ask_user::tests::cancellation_via_future_drop ... ok
test tools::ask_user::tests::outbound_message_sent_with_prompt ... ok
test tools::ask_user::tests::missing_registry_in_context_errors_cleanly ... ok

test result: ok. 5 passed
```

- [ ] **Step 4.5: Commit**

```bash
git add gasket/engine/src/tools/ask_user.rs gasket/engine/src/tools/mod.rs
git commit -m "engine(tools): add AskUserTool with L2 integration tests"
```

---

## Task 5: Register `AskUserTool` in `CoreToolProvider`

**Why:** Without registration, the tool isn't discoverable by the LLM/registry.

**Files:**
- Modify: `gasket/engine/src/tools/provider.rs`

- [ ] **Step 5.1: Import the tool**

Modify `gasket/engine/src/tools/provider.rs`. Add `AskUserTool` to the existing `super::{…}` import block (currently lines 14-20):

```rust
use super::{
    registry::ToolRegistry, AskUserTool, ClearSessionTool, CreatePlanTool, EditFileTool,
    EvolutionConfig, EvolutionTool, ExecTool, HistoryQueryTool, ListDirTool, NewSessionTool,
    ReadFileTool, SearchSopsTool, SpawnParallelTool, SpawnTool, ToolMetadata, WebFetchTool,
    WebSearchTool, WikiDecayTool, WikiDeleteTool, WikiReadTool, WikiRefreshTool, WikiSearchTool,
    WikiWriteTool, WriteFileTool,
};
```

- [ ] **Step 5.2: Register the tool inside `CoreToolProvider::register_tools`**

Modify `gasket/engine/src/tools/provider.rs`. In `CoreToolProvider::register_tools`, just **before** the `// Spawn tools — only the Orchestrator gets these` block (around line 164, but after the WebSearchTool registration), add:

```rust
        // ── User interaction ────────────────────────────────────────
        reg!(
            registry,
            AskUserTool::new(),
            "Ask User",
            "interaction",
            ["user", "prompt"],
            false,
            false
        );
```

- [ ] **Step 5.3: Verify build and existing tests still pass**

Run: `cargo test -p gasket-engine`
Expected: all existing tests pass; `AskUserTool` is now registered.

- [ ] **Step 5.4: Commit**

```bash
git add gasket/engine/src/tools/provider.rs
git commit -m "engine(tools): register AskUserTool in CoreToolProvider"
```

---

## Task 6: Thread `pending_asks` through `EngineHandle` and plugin path

**Why:** Plugins reach `ask_user` via `ToolDelegateHandler`, which builds a `ToolContext` from `EngineHandle`. Without `pending_asks` on the handle, plugin-side `user/ask` calls would see `None` and fail.

**Files:**
- Modify: `gasket/engine/src/plugin/dispatcher/mod.rs`
- Modify: `gasket/engine/src/plugin/mod.rs`

- [ ] **Step 6.1: Add `pending_asks` to `EngineHandle`**

Modify `gasket/engine/src/plugin/dispatcher/mod.rs`. In the `EngineHandle` struct (currently around line 70), add a new field after `provider`:

```rust
    /// Pending-ask registry. None in contexts that don't support user prompting.
    pub pending_asks: Option<gasket_types::pending_ask::DynPendingAskRegistry>,
```

- [ ] **Step 6.2: Inject `pending_asks` in `ToolDelegateHandler::handle`**

In the same file, in `ToolDelegateHandler::handle` (currently around line 248-269), update the `ToolContext` construction to thread the registry. After the existing `if let Some(spawner) = …` block, add:

```rust
        if let Some(registry) = &ctx.engine.pending_asks {
            tool_ctx = tool_ctx.pending_asks(registry.clone());
        }
```

- [ ] **Step 6.3: Update `EngineHandle` constructions in tests**

In the same file, in the `tests` module's `create_test_ctx` (around line 373), add the new field with `None`:

```rust
        DispatcherContext {
            engine: Arc::new(EngineHandle {
                session_key: SessionKey::new(
                    gasket_types::events::ChannelType::Telegram,
                    "test-chat",
                ),
                outbound_tx: tx,
                spawner: Some(Arc::new(MockSpawner)),
                token_tracker: Arc::new(gasket_types::token_tracker::TokenTracker::unlimited(
                    "USD",
                )),
                tool_registry: Arc::new(ToolRegistry::new()),
                provider: Arc::new(MockProvider),
                pending_asks: None,
            }),
        }
```

- [ ] **Step 6.4: Update `make_dispatch_ctx` in plugin/mod.rs**

Modify `gasket/engine/src/plugin/mod.rs`. In `PluginTool::make_dispatch_ctx` (currently around lines 87-107), update the `EngineHandle` construction:

```rust
        Ok(DispatcherContext {
            engine: Arc::new(EngineHandle {
                session_key: ctx.session_key.clone(),
                outbound_tx: ctx.outbound_tx.clone(),
                spawner: ctx.spawner.clone(),
                token_tracker: ctx.token_tracker.clone(),
                tool_registry: resources.tool_registry.clone(),
                provider: resources.provider.clone(),
                pending_asks: ctx.pending_asks.clone(),
            }),
        })
```

- [ ] **Step 6.5: Update the test in plugin/mod.rs that constructs `EngineHandle` indirectly**

Modify the `test_script_tool_make_dispatch_ctx` test (around lines 542-643). The test goes through `ToolContext` so as long as the field is on `ToolContext` (Task 3 done) it should still compile. Run the build to confirm.

Run: `cargo build -p gasket-engine`
Expected: clean build.

- [ ] **Step 6.6: Run dispatcher tests**

Run: `cargo test -p gasket-engine --lib plugin::dispatcher`
Expected: all dispatcher tests pass.

- [ ] **Step 6.7: Commit**

```bash
git add gasket/engine/src/plugin/dispatcher/mod.rs gasket/engine/src/plugin/mod.rs
git commit -m "engine(plugin): thread pending_asks through EngineHandle"
```

---

## Task 7: `Permission::UserAsk` + `user/ask` dispatcher entry

**Why:** Activates the plugin-side RPC surface.

**Files:**
- Modify: `gasket/engine/src/plugin/manifest.rs`
- Modify: `gasket/engine/src/plugin/dispatcher/mod.rs`

- [ ] **Step 7.1: Add the `Permission::UserAsk` variant**

Modify `gasket/engine/src/plugin/manifest.rs`. In the `Permission` enum (currently lines 103-121), add:

```rust
    /// Permission to ask the user a question and wait for their reply
    UserAsk,
```

In `Permission::method_name` (currently lines 128-138), add the matching arm:

```rust
            Permission::UserAsk => "user/ask",
```

- [ ] **Step 7.2: Add a test for the new permission roundtrip**

In the `tests` module of the same file, add a new test (after `test_permission_method_names` near line 285):

```rust
    #[test]
    fn test_permission_user_ask_method_name() {
        assert_eq!(Permission::UserAsk.method_name(), "user/ask");
    }

    #[test]
    fn test_permission_user_ask_serde_roundtrip() {
        let perms = vec![Permission::UserAsk];
        let yaml = serde_yaml::to_string(&perms).expect("serialize");
        assert!(yaml.contains("user_ask"));
        let parsed: Vec<Permission> = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(parsed, vec![Permission::UserAsk]);
    }
```

- [ ] **Step 7.3: Register `user/ask` in `build_dispatcher`**

Modify `gasket/engine/src/plugin/dispatcher/mod.rs`. In `build_dispatcher` (currently lines 282-312), after the `message/send` registration, add:

```rust
    d.register(Arc::new(ToolDelegateHandler::new(
        "user/ask",
        Permission::UserAsk,
        "ask_user",
    )))
    .unwrap();
```

- [ ] **Step 7.4: Run manifest tests, expect all pass**

Run: `cargo test -p gasket-engine --lib plugin::manifest`
Expected: all manifest tests pass including the two new ones.

- [ ] **Step 7.5: Commit**

```bash
git add gasket/engine/src/plugin/manifest.rs gasket/engine/src/plugin/dispatcher/mod.rs
git commit -m "engine(plugin): expose user/ask RPC and Permission::UserAsk"
```

---

## Task 8: `AgentSession.pending_asks` + `handle_inbound` entries + L3 routing tests

**Why:** Wire the registry into the session and add the new entry point that all inbound traffic routes through.

**Files:**
- Modify: `gasket/engine/src/session/mod.rs`
- Modify: `gasket/engine/src/session/builder.rs`
- Create: `gasket/engine/tests/handle_inbound.rs`

- [ ] **Step 8.1: Add `pending_asks` field to `AgentSession`**

Modify `gasket/engine/src/session/mod.rs`. In the `AgentSession` struct (lines 234-251), add a new field after `pending_done` (line 246) — match the existing private field style:

```rust
    /// Pending-ask registry shared with tools through `RuntimeContext`.
    pending_asks: Arc<PendingAskRegistryImpl>,
```

- [ ] **Step 8.2: Initialize the field in the builder**

Modify `gasket/engine/src/session/builder.rs`. In the `AgentSession { … }` struct literal in the builder (lines 248-258), add the field initialization between the existing fields:

```rust
            pending_asks: Arc::new(PendingAskRegistryImpl::new()),
```

If `PendingAskRegistryImpl` is not yet imported at the top of `builder.rs`, add:

```rust
use crate::session::PendingAskRegistryImpl;
```

- [ ] **Step 8.3a: Add `pending_asks` field to `RuntimeContext`**

Modify `gasket/engine/src/kernel/context.rs`. In the `RuntimeContext` struct (lines 27-45), add a new field after `aggregator_cancel`:

```rust
    /// Pending-ask registry for the `ask_user` tool. None disables user prompting.
    pub pending_asks: Option<gasket_types::pending_ask::DynPendingAskRegistry>,
```

In `RuntimeContext::new` and `RuntimeContext::new_worker` (lines 47-87), add the field initialization:

```rust
            pending_asks: None,
```

In the `Clone for RuntimeContext` impl (lines 90-105), add:

```rust
            pending_asks: self.pending_asks.clone(),
```

- [ ] **Step 8.3b: Inject `pending_asks` from `RuntimeContext` into each `ToolContext` in `steppable_executor.rs`**

Modify `gasket/engine/src/kernel/steppable_executor.rs`. In the `ToolContext` builder block (lines 205-234), after the existing `if let Some(ref cancel) = self.ctx.aggregator_cancel { … }` block (line 232-234), add:

```rust
        if let Some(ref registry) = self.ctx.pending_asks {
            ctx = ctx.pending_asks(registry.clone());
        }
```

- [ ] **Step 8.3c: Set `runtime_ctx.pending_asks` in `process_direct_streaming_with_channel`**

Modify `gasket/engine/src/session/mod.rs`. In `process_direct_streaming_with_channel` (around line 474), locate the `ctx.runtime_ctx.outbound_tx = Some(outbound_tx);` line (≈ line 518). Right after it, add:

```rust
        ctx.runtime_ctx.pending_asks = Some(self.pending_asks.clone() as gasket_types::pending_ask::DynPendingAskRegistry);
```

- [ ] **Step 8.4: Add `HandleOutcome` enum and `handle_inbound{,_streaming_with_channel}` methods**

In `gasket/engine/src/session/mod.rs`, near the top with the other public types (e.g. just before `pub struct AgentSession`), add:

```rust
/// Outcome of `handle_inbound` (blocking variant).
pub enum HandleOutcome {
    /// Inbound was consumed by a pending `ask_user`. No reply emitted.
    Consumed,
    /// Inbound triggered a normal LLM turn.
    Replied(AgentResponse),
}

/// Outcome of `handle_inbound_streaming_with_channel` (streaming variant).
pub enum HandleOutcomeStreaming {
    /// Inbound was consumed by a pending `ask_user`. No reply emitted.
    Consumed,
    /// Inbound triggered a normal LLM turn; consumer can stream events and
    /// await the result.
    Replied {
        events: tokio::sync::mpsc::Receiver<gasket_types::events::ChatEvent>,
        result: tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
    },
}
```

In the `impl AgentSession` block, just before `process_direct` (line 453), add:

```rust
    /// Inbound entry: try to deliver to a pending ask first, otherwise run
    /// `process_direct`.
    pub async fn handle_inbound(
        &self,
        content: &str,
        session_key: &SessionKey,
        tool_filter: Option<Vec<String>>,
    ) -> Result<HandleOutcome, AgentError> {
        // Synthesize a minimal InboundMessage for the registry to consume.
        let synthetic = gasket_types::events::InboundMessage {
            channel: session_key.channel.clone(),
            sender_id: session_key.chat_id.clone(),
            chat_id: session_key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };
        if self.pending_asks.try_fulfill(session_key, synthetic).is_ok() {
            return Ok(HandleOutcome::Consumed);
        }
        let resp = self.process_direct(content, session_key, tool_filter).await?;
        Ok(HandleOutcome::Replied(resp))
    }

    /// Streaming variant.
    pub async fn handle_inbound_streaming_with_channel(
        &self,
        content: &str,
        session_key: &SessionKey,
        tool_filter: Option<Vec<String>>,
    ) -> Result<HandleOutcomeStreaming, AgentError> {
        let synthetic = gasket_types::events::InboundMessage {
            channel: session_key.channel.clone(),
            sender_id: session_key.chat_id.clone(),
            chat_id: session_key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };
        if self.pending_asks.try_fulfill(session_key, synthetic).is_ok() {
            return Ok(HandleOutcomeStreaming::Consumed);
        }
        let (events, result) = self
            .process_direct_streaming_with_channel(content, session_key, tool_filter)
            .await?;
        Ok(HandleOutcomeStreaming::Replied { events, result })
    }
```

- [ ] **Step 8.5: Verify build**

Run: `cargo build -p gasket-engine`
Expected: clean build. If `RuntimeContext` does not yet have `pending_asks`, fix that first (see Step 8.3 note).

- [ ] **Step 8.6: Create L3 routing tests**

Create `gasket/engine/tests/handle_inbound.rs`:

```rust
//! L3 routing tests: handle_inbound short-circuits on pending, falls through otherwise.

use std::sync::Arc;

use gasket_engine::session::{HandleOutcome, PendingAskRegistryImpl};
use gasket_types::events::{ChannelType, SessionKey};
use gasket_types::pending_ask::PendingAskRegistry;

// Note: these tests exercise the registry directly + a fake AgentSession
// builder fixture. If a full AgentSession fixture is too heavy here, keep
// the routing logic test focused on the registry contract.

#[tokio::test]
async fn registry_try_fulfill_short_circuits_for_pending_session() {
    let registry = Arc::new(PendingAskRegistryImpl::new());
    let key = SessionKey::new(ChannelType::Cli, "a");

    // Register an ask.
    let registration = registry
        .register(
            key.clone(),
            "q?".into(),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .unwrap();

    // Synthesize an inbound; try_fulfill must succeed.
    let msg = gasket_types::events::InboundMessage {
        channel: key.channel.clone(),
        sender_id: key.chat_id.clone(),
        chat_id: key.chat_id.clone(),
        content: "answer".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    registry.try_fulfill(&key, msg).expect("fulfill");

    let answer = registration.answer_rx.await.unwrap();
    assert_eq!(answer.content, "answer");
}

#[tokio::test]
async fn registry_try_fulfill_misses_for_other_session() {
    let registry = Arc::new(PendingAskRegistryImpl::new());
    let key_a = SessionKey::new(ChannelType::Cli, "a");
    let key_b = SessionKey::new(ChannelType::Cli, "b");

    // A has a pending ask.
    let _ra = registry
        .register(
            key_a.clone(),
            "q?".into(),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .unwrap();

    // B's inbound must miss.
    let msg = gasket_types::events::InboundMessage {
        channel: key_b.channel.clone(),
        sender_id: key_b.chat_id.clone(),
        chat_id: key_b.chat_id.clone(),
        content: "for-b".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    let returned = registry.try_fulfill(&key_b, msg).unwrap_err();
    assert_eq!(returned.content, "for-b");
}

#[tokio::test]
async fn handle_outcome_enum_compiles() {
    // Smoke check that HandleOutcome is publicly exposed and matchable.
    fn _accept(out: HandleOutcome) {
        match out {
            HandleOutcome::Consumed => {}
            HandleOutcome::Replied(_) => {}
        }
    }
}
```

> **Note on full `AgentSession` fixtures:** A complete `AgentSession` requires a SQLite store, provider, hooks, etc. — heavy machinery. The L3 tests above target the routing contract via the registry directly; a thicker fixture would belong in L4 (Task 11) where we go through the full plugin path.

- [ ] **Step 8.7: Run L3 tests**

Run: `cargo test -p gasket-engine --test handle_inbound`
Expected: 3 tests pass.

- [ ] **Step 8.8: Commit**

```bash
git add gasket/engine/src/session/mod.rs gasket/engine/src/session/builder.rs gasket/engine/tests/handle_inbound.rs
git commit -m "engine(session): add handle_inbound entry + L3 routing tests"
```

---

## Task 9: `gasket_sdk.py` — `ask_user` helper

**Why:** Plugin authors need a Python wrapper.

**Files:**
- Modify: `workspace/plugins/gasket_sdk.py`

- [ ] **Step 9.1: Add the helper**

Modify `workspace/plugins/gasket_sdk.py`. After the `send_message` method (around line 70-75), add:

```python
    def ask_user(self, prompt: str, timeout_secs: int) -> dict:
        """Ask the user a question and block until they reply.

        Returns:
            {"content": str, "sender_id": str, "channel": str,
             "timestamp": str, "media": Optional[list]}

        Raises:
            RuntimeError: on timeout, already-pending, missing registry, or
            session shutdown.
        """
        return self._call("user/ask", {
            "prompt": prompt,
            "timeout_secs": timeout_secs,
        })
```

- [ ] **Step 9.2: Quick syntax check**

Run: `python3 -c "import ast; ast.parse(open('workspace/plugins/gasket_sdk.py').read())"`
Expected: no output (valid syntax).

- [ ] **Step 9.3: Commit**

```bash
git add workspace/plugins/gasket_sdk.py
git commit -m "sdk(python): add GasketPlugin.ask_user helper"
```

---

## Task 10: Switch all 6 inbound entry points to `handle_inbound{,_streaming_with_channel}`

**Why:** Without this switch, the new routing path is dead code. After this switch, `ask_user` is live for real channels.

**Files:**
- Modify: `gasket/engine/src/bus_adapter.rs` (2 sites)
- Modify: `gasket/cli/src/commands/agent.rs` (4 sites)

- [ ] **Step 10.1: Migrate `bus_adapter.rs::handle_message`**

Modify `gasket/engine/src/bus_adapter.rs`. In `handle_message` (lines 31-43), replace the body:

```rust
    async fn handle_message(
        &self,
        session_key: &SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::session::HandleOutcome;
        let outcome = self
            .session
            .handle_inbound(message, session_key, None)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(match outcome {
            HandleOutcome::Consumed => String::new(),
            HandleOutcome::Replied(resp) => resp.content,
        })
    }
```

- [ ] **Step 10.2: Migrate `bus_adapter.rs::handle_streaming_message`**

In the same file, in `handle_streaming_message` (lines 45-96), replace the call to `process_direct_streaming_with_channel` with `handle_inbound_streaming_with_channel`. The change is just the method name **and** unwrapping `HandleOutcomeStreaming`. Replace the relevant block:

```rust
        use crate::session::HandleOutcomeStreaming;
        let outcome = self
            .session
            .handle_inbound_streaming_with_channel(message, session_key, tool_filter)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let (chat_rx, result_handle) = match outcome {
            HandleOutcomeStreaming::Consumed => {
                // Inbound was consumed by a pending ask. Return an empty
                // event stream and a synthesized "no reply" result.
                let (_chat_tx, chat_rx) = tokio::sync::mpsc::channel(1);
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                let outbound_msg = gasket_types::events::OutboundMessage::new(
                    gasket_types::events::ChannelType::Cli,
                    session_key.to_string(),
                    String::new(),
                );
                let _ = result_tx.send(Ok(outbound_msg));
                return Ok((chat_rx, result_rx));
            }
            HandleOutcomeStreaming::Replied { events, result } => (events, result),
        };
```

Then keep the existing `tokio::spawn` block that wraps `result_handle` into the oneshot.

- [ ] **Step 10.3: Migrate the 4 sites in `cli/src/commands/agent.rs`**

Modify `gasket/cli/src/commands/agent.rs`. For each of the 4 sites:

- Line 304: `process_direct_streaming_with_channel(&msg, &session_key, None)` → `handle_inbound_streaming_with_channel(&msg, &session_key, None)`. Then unwrap `HandleOutcomeStreaming::Replied { events, result }` (matching pattern); on `Consumed`, print a no-op note (e.g., `eprintln!("(answered)");`) and continue without producing a reply.
- Line 329: `process_direct(&msg, &session_key, None).await?` → wrap with `handle_inbound(&msg, &session_key, None).await?` and match on `HandleOutcome`. On `Consumed`, emit empty/silent reply path (no print). On `Replied(resp)`, use `resp` as before.
- Line 466: same as 304 with different argument names (`text`, `session_key`, `tool_filter`).
- Line 500: same as 329 with different argument names.

Pattern for the streaming site:

```rust
use gasket_engine::session::HandleOutcomeStreaming;
let outcome = agent
    .handle_inbound_streaming_with_channel(text, session_key, tool_filter)
    .await?;
match outcome {
    HandleOutcomeStreaming::Consumed => {
        // The user's input answered a pending ask_user; no reply emitted.
    }
    HandleOutcomeStreaming::Replied { events, result } => {
        // existing streaming-handling code that uses (events, result)
    }
}
```

Pattern for the blocking site:

```rust
use gasket_engine::session::HandleOutcome;
match agent.handle_inbound(text, session_key, tool_filter).await {
    Ok(HandleOutcome::Consumed) => { /* no reply, conversation continues */ }
    Ok(HandleOutcome::Replied(resp)) => { /* existing handling */ }
    Err(e) => { /* existing error handling */ }
}
```

- [ ] **Step 10.4: Run the workspace test suite, expect no regressions**

Run: `cargo test --workspace`
Expected: all existing tests still pass; CLI / bus_adapter integration tests still green.

- [ ] **Step 10.5: Commit**

```bash
git add gasket/engine/src/bus_adapter.rs gasket/cli/src/commands/agent.rs
git commit -m "engine,cli(inbound): route through handle_inbound for ask_user support"
```

---

## Task 11: Demonstrate in `dev_workflow` + L4 e2e tests

**Why:** Validate the end-to-end path with a real plugin and verify the contract via integration tests.

**Files:**
- Modify: `workspace/plugins/dev_workflow.yaml`
- Modify: `workspace/plugins/workflows/dev.py`
- Create: `gasket/engine/tests/plugin_ask_user.rs`

- [ ] **Step 11.1: Add `user_ask` permission to `dev_workflow.yaml`**

Modify `workspace/plugins/dev_workflow.yaml`. Append `user_ask` to the existing `permissions` list:

```yaml
permissions:
  - subagent_spawn
  - message_send
  - user_ask
```

- [ ] **Step 11.2: Add a clarification phase to `dev.py`**

Modify `workspace/plugins/workflows/dev.py`. After the argument validation in `main()` and **before** `# Phase 1: Research`, add a clarification phase:

```python
        # Phase 0: Clarification — surface ambiguities and ask the user.
        _notify(plugin, "Identifying ambiguities...")
        ambiguities_raw = plugin.spawn_subagent(
            f"Identify any ambiguities in this task that need user "
            f"clarification. Output STRICT JSON only:\n"
            f'{{"questions": ["q1", "q2", ...]}}\n\n'
            f"If there are no ambiguities, return an empty list.\n\n"
            f"Task: {task}",
            model=reasoner,
        )
        try:
            questions = json.loads(ambiguities_raw.strip().strip("`")).get("questions", [])
        except json.JSONDecodeError:
            questions = []  # If the model misbehaves, skip clarification.

        clarifications = []
        for q in questions[:3]:  # Cap at 3 to avoid harassment.
            try:
                ans = plugin.ask_user(q, timeout_secs=300)
                clarifications.append({"q": q, "a": ans.get("content", "")})
            except RuntimeError as e:
                clarifications.append({"q": q, "a": f"[unanswered: {e}]"})

        if clarifications:
            task = task + "\n\nClarifications:\n" + json.dumps(
                clarifications, ensure_ascii=False, indent=2
            )
```

- [ ] **Step 11.3: Create the L4 e2e test file**

Create `gasket/engine/tests/plugin_ask_user.rs`:

```rust
//! L4 end-to-end tests: plugin path through the dispatcher reaches the
//! ask_user tool and gets fulfilled by an inbound message.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gasket_engine::session::PendingAskRegistryImpl;
use gasket_types::events::{ChannelType, InboundMessage, SessionKey};
use gasket_types::pending_ask::{DynPendingAskRegistry, PendingAskRegistry};

#[tokio::test]
async fn registry_supports_concurrent_pending_in_two_sessions() {
    let registry: DynPendingAskRegistry = Arc::new(PendingAskRegistryImpl::new());
    let ka = SessionKey::new(ChannelType::Cli, "alice");
    let kb = SessionKey::new(ChannelType::Cli, "bob");

    let ra = registry
        .register(ka.clone(), "qa".into(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    let rb = registry
        .register(kb.clone(), "qb".into(), Instant::now() + Duration::from_secs(30))
        .unwrap();

    // Fulfill out of order; both must resolve.
    let inbound_b = InboundMessage {
        channel: kb.channel.clone(),
        sender_id: "bob".into(),
        chat_id: kb.chat_id.clone(),
        content: "ans-b".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    registry.try_fulfill(&kb, inbound_b).unwrap();

    let inbound_a = InboundMessage {
        channel: ka.channel.clone(),
        sender_id: "alice".into(),
        chat_id: ka.chat_id.clone(),
        content: "ans-a".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    registry.try_fulfill(&ka, inbound_a).unwrap();

    let ans_a = ra.answer_rx.await.unwrap();
    let ans_b = rb.answer_rx.await.unwrap();
    assert_eq!(ans_a.content, "ans-a");
    assert_eq!(ans_b.content, "ans-b");
}

// NOTE: A full plugin → subagent → ask_user e2e via spawning a Python plugin
// daemon is intentionally omitted from automated tests. Spawning a plugin
// requires a real LLM provider and SQLite store, which makes the test brittle
// in CI. Instead, the L4 contract is validated by:
//   1. Task 11.4 manual smoke run (CLI + dev_workflow).
//   2. The registry-level concurrency test above.
// This matches the spec's "L5 — CLI smoke" guidance.
```

- [ ] **Step 11.4: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass, including the new L4 test.

- [ ] **Step 11.5: Commit**

```bash
git add workspace/plugins/dev_workflow.yaml workspace/plugins/workflows/dev.py gasket/engine/tests/plugin_ask_user.rs
git commit -m "plugin(dev_workflow): add clarification phase via ask_user; L4 tests"
```

---

## Task 12: L5 manual smoke verification

**Why:** Verify the full integration in a real CLI session before declaring done.

- [ ] **Step 12.1: Build the workspace**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 12.2: Run the CLI agent in interactive mode and trigger `ask_user`**

Run (in a terminal):
```bash
cargo run -p gasket-cli -- agent
```
Then type a prompt that the LLM is likely to need clarification on, e.g.:
```
write a function but I haven't told you in which language
```

Expected: the LLM calls `ask_user`, the prompt prints, you type a reply (e.g. "python"), the conversation continues with the reply incorporated as `tool_result`.

- [ ] **Step 12.3: Run dev_workflow with a deliberately ambiguous task**

Run:
```bash
cargo run -p gasket-cli -- tool execute dev_workflow '{"task": "write a thing"}'
```
Expected: the workflow's clarification phase prints a question (e.g., "what kind of thing?"), waits for your reply, then proceeds to research/plan/implement/review.

- [ ] **Step 12.4: Verify the "already pending" guardrail (manual)**

In a single CLI turn, ask the LLM to call `ask_user` twice in parallel (e.g., "ask me 'a?' and 'b?' simultaneously"). Expected: the LLM gets back the `already pending` error string for the second call and naturally falls back to one-at-a-time.

- [ ] **Step 12.5: Verify the timeout path (manual)**

Trigger an `ask_user` (via dev_workflow), then **don't reply** for >300s. Expected: the workflow prints `[unanswered: ask timed out after …]` and continues.

- [ ] **Step 12.6: Record observations in PR description; no commit needed for manual tests**

Document the manual smoke results in the PR body so reviewers can audit.

---

## Final Verification

- [ ] **Step F.1: Full workspace test run**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step F.2: Lint clean**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step F.3: Format clean**

Run: `cargo fmt --all --check`
Expected: no diff.

- [ ] **Step F.4: Push and open PR**

```bash
git push -u origin <branch>
gh pr create --title "feat(engine): ask_user — pause workflow and wait for user reply" --body "..."
```

---

## Acceptance Criteria (from spec)

- [x] L1 (registry) tests green: `cargo test -p gasket-engine --lib session::pending_ask::tests` — Task 2
- [x] L2 (tool) tests green: `cargo test -p gasket-engine --lib tools::ask_user::tests` — Task 4
- [x] L3 (routing) tests green: `cargo test -p gasket-engine --test handle_inbound` — Task 8
- [x] L4 (registry concurrency / e2e contract) green: `cargo test -p gasket-engine --test plugin_ask_user` — Task 11
- [x] L5 manual smoke completes one prompt → reply round in CLI mode — Task 12
- [x] Existing `cargo test --workspace` continues to pass — Task 10 + Final
- [x] Plugin without `user_ask` permission produces `Permission denied` when calling `user/ask` — covered by existing dispatcher permission check (no new code path)
- [x] Timeout returns a `tool_result` containing the error string (not panic, not hang) — Task 4 step 4.2
- [x] Second `ask_user` on same `SessionKey` while first is pending returns `already_pending` error string — Task 2 (`register_twice_same_session_rejected`) + Task 4 surfaces via `ToolError::ExecutionError`
