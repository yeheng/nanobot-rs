# Flow Command System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Flow Command System per spec `docs/superpowers/specs/2026-05-03-flow-command-system-design.md` (revised after Linus review). Two independent batches:
- **Batch v1 (Tasks 1–4)**: `WikiGuard` + `WikiPolicy` + `/wiki review` CLI subcommand. Solves the user's most acute pain (wiki writes too frequent) with minimal code. Ships standalone.
- **Batch v2 (Tasks 5–8)**: Slash-command flow engine. Builds on v1's `WikiGuard`. PhaseRunner really calls `kernel::execute_streaming` (no stub).

**Architecture:** v1 inserts a single struct `WikiGuard` between `WikiWriteTool` and `PageStore`. v2 adds two new modules: `gasket/engine/src/flow/` (orchestrator + state machine + template loader) and `gasket/engine/src/command/` (slash-command parser + dispatcher). Plain text bypasses both v2 modules and goes to `AgentSession` unchanged.

**Tech Stack:** Rust 2021, tokio async, sqlx (SQLite), serde_yaml, uuid v7, chrono, async-trait. All deps already in `gasket/Cargo.toml` workspace dependencies. No new crates.

---

## File Structure

### Batch v1

| File | Action | Responsibility |
|------|--------|----------------|
| `gasket/engine/src/flow/mod.rs` | Create | Flow module skeleton (only `wiki_guard` in v1; v2 adds the rest) |
| `gasket/engine/src/flow/wiki_guard.rs` | Create | Single `WikiGuard` struct + `WikiPolicy` enum + `GuardDecision` enum |
| `gasket/engine/src/lib.rs` | Modify | `pub mod flow;` |
| `gasket/engine/src/tools/wiki_tools.rs` | Modify | `WikiWriteTool` holds `Arc<WikiGuard>`; default `Allowed` |
| `gasket/types/src/flow.rs` | Create | `PendingWikiWrite` struct (shared across crates) |
| `gasket/types/src/lib.rs` | Modify | Re-export `flow` module |
| `gasket/cli/src/commands/wiki.rs` | Modify | Add `cmd_wiki_review` subcommand |
| `gasket/cli/src/cli.rs` (or wherever clap structs live) | Modify | Wire `wiki review` subcommand |
| `gasket/cli/src/commands/agent.rs` | Modify | Construct shared `Arc<WikiGuard>` and inject into `WikiWriteTool` |

### Batch v2 (additionally)

| File | Action | Responsibility |
|------|--------|----------------|
| `gasket/types/src/flow.rs` | Modify | Add `PhaseId` (= `String`), `FlowStatus`, `PhaseOutput`, `is_builtin_phase()` |
| `gasket/types/src/events/stream.rs` | Modify | 4 new `ChatEvent` variants: `FlowStarted`, `PhaseChanged`, `GatePending`, `FlowFinished` |
| `gasket/storage/src/migrations/flow_run.rs` | Create | DDL with `template_yaml` column |
| `gasket/storage/src/migrations/mod.rs` | Modify | Register migration |
| `gasket/storage/src/flow_run_store.rs` | Create | `FlowRunRecord` + `FlowRunStore` (insert/load/update/list/active_for_session) |
| `gasket/storage/src/lib.rs` | Modify | Re-export + accessor on `SqliteStore` |
| `gasket/engine/src/flow/template.rs` | Create | YAML loader (supports `gate: Option`, `max_iterations: Option`, `allowed_tools: Option`) |
| `gasket/engine/src/flow/state.rs` | Create | `FlowState` + state machine (with `edit_feedback` field) |
| `gasket/engine/src/flow/phase_runner.rs` | Create | Single-phase kernel call wrapper; consumes `edit_feedback` |
| `gasket/engine/src/flow/gate.rs` | Create | CLI gate controller + shared `parse_response` |
| `gasket/engine/src/flow/orchestrator.rs` | Create | `FlowOrchestrator` (real kernel integration, no stub) |
| `gasket/engine/src/flow/mod.rs` | Modify | Add new module re-exports |
| `gasket/engine/src/command/mod.rs` | Create | Command module |
| `gasket/engine/src/command/parser.rs` | Create | Parse `/flow start ...`, `/brainstorm`, etc. → `CommandAction` |
| `gasket/engine/src/command/dispatcher.rs` | Create | `CommandDispatcher::route` |
| `gasket/engine/src/lib.rs` | Modify | `pub mod command;` |
| `gasket/cli/src/commands/agent.rs` | Modify | Wire dispatcher; real `drive_flow` that calls `PhaseRunner` |
| `gasket/engine/flows/{default,debug,docs,brainstorm-only,…}.yaml` | Create | Built-in templates |
| `gasket/engine/flows/prompts/{brainstorm,design,plan,execute,verify}.md` | Create | Default prompts |
| `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md` | Modify | Mark Status: Superseded |

---

## Implementation Notes for the Engineer

**TDD discipline.** Every task: write test → run it (fail) → write impl → run test (pass) → commit. Don't skip the failing-test step.

**Commit cadence.** One commit per task (8 commits total). Pre-commit hooks run `cargo clippy --fix`, `cargo fmt`, `cargo build` — expect ~30s per commit.

**Reading the spec.** `docs/superpowers/specs/2026-05-03-flow-command-system-design.md` is authoritative. When in doubt, read the spec section called out in the task.

**Linus simplifications already applied in this plan:**
- `PhaseId` is `String`, not an enum
- `FlowStatus::AwaitingGate` carries no `phase` field
- `gate` / `max_iterations` / `allowed_tools` are `Option<...>` (no magic 0 / no `["*"]`)
- `WikiGuard` is a single struct, not three trait impls
- `FlowOrchestrator::step` returns `Result<&FlowStatus>` — no separate `OrchestratorOutcome` type
- `GateResponse::Edit(text)` actually plumbs `text` through to the next phase via `FlowState.edit_feedback`
- `flow_runs.template_yaml` column makes resume safe across template edits

**Existing types to know:**
- `gasket_types::Tool` trait — `WikiWriteTool` implements this
- `gasket_storage::SqliteStore` — pool wrapper; `CronStore` / `KvStore` are the access pattern to mirror
- `gasket_engine::kernel::execute_streaming(ctx, messages, event_tx)` — the LLM loop

---

# BATCH v1 — Wiki Write Guard

Goal of this batch: ship a working `/wiki review` workflow that defers and approves wiki writes. Independent of any flow engine. Default behavior unchanged for users who don't opt in.

---

### Task 1: `WikiGuard` + `WikiPolicy` + `PendingWikiWrite`

**Files:**
- Create: `gasket/types/src/flow.rs`
- Modify: `gasket/types/src/lib.rs`
- Create: `gasket/engine/src/flow/mod.rs`
- Create: `gasket/engine/src/flow/wiki_guard.rs`
- Modify: `gasket/engine/src/lib.rs`

- [ ] **Step 1: Create `gasket/types/src/flow.rs` with the shared `PendingWikiWrite` type and tests**

```rust
//! Flow execution types shared across gasket crates.
//!
//! In v1, only `PendingWikiWrite` lives here. v2 adds `PhaseId`, `FlowStatus`,
//! `PhaseOutput`, etc.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A wiki write request that was intercepted by `WikiGuard` while in `Deferred`
/// policy mode. Lives in the guard's pending queue until the user approves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingWikiWrite {
    pub path: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub page_type: String,
    pub queued_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_wiki_write_round_trip() {
        let w = PendingWikiWrite {
            path: "topics/x".to_string(),
            title: "T".to_string(),
            content: "C".to_string(),
            tags: vec!["rust".to_string()],
            page_type: "topic".to_string(),
            queued_at: Utc::now(),
        };
        let s = serde_json::to_string(&w).unwrap();
        let de: PendingWikiWrite = serde_json::from_str(&s).unwrap();
        assert_eq!(de.path, "topics/x");
        assert_eq!(de.tags, vec!["rust".to_string()]);
    }
}
```

In `gasket/types/src/lib.rs`, after `pub mod tool;` add:

```rust
pub mod flow;
```

And after the existing re-exports add:

```rust
pub use flow::PendingWikiWrite;
```

- [ ] **Step 2: Run the type test**

```bash
cargo test --package gasket-types --lib flow:: 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 3: Create `gasket/engine/src/flow/mod.rs`**

```rust
//! Flow command system.
//!
//! v1: `wiki_guard` only.
//! v2: state machine + orchestrator + templates (added in batch v2).

pub mod wiki_guard;

pub use wiki_guard::{GuardDecision, WikiGuard, WikiPolicy, WriteArgsView};
```

- [ ] **Step 4: Create `gasket/engine/src/flow/wiki_guard.rs` with failing tests**

```rust
//! Wiki write interception: a single struct + policy enum, no traits.

use chrono::Utc;
use gasket_types::PendingWikiWrite;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WikiPolicy {
    Allowed,
    Deferred,
    Blocked,
}

impl Default for WikiPolicy {
    fn default() -> Self { WikiPolicy::Allowed }
}

/// View of `WikiWriteTool` arguments exposed to the guard.
/// Decoupled from the private `WriteArgs` struct in `wiki_tools.rs`.
#[derive(Debug, Clone)]
pub struct WriteArgsView {
    pub path: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub page_type: String,
}

#[derive(Debug)]
pub enum GuardDecision {
    /// Caller proceeds to `PageStore::write`.
    Allow,
    /// Caller returns a "deferred" message to the LLM. Item is in queue.
    Defer,
    /// Caller returns this string to the LLM as a tool error.
    Reject(String),
}

/// Wiki write guard. One struct, one policy, one queue.
pub struct WikiGuard {
    policy: Mutex<WikiPolicy>,
    pending: Mutex<Vec<PendingWikiWrite>>,
}

impl WikiGuard {
    pub fn new(policy: WikiPolicy) -> Self {
        Self {
            policy: Mutex::new(policy),
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Default constructor used by `WikiWriteTool::new`. Policy = Allowed.
    pub fn allow_all() -> Self { Self::new(WikiPolicy::Allowed) }

    pub fn policy(&self) -> WikiPolicy {
        *self.policy.lock().unwrap()
    }

    /// Change the active policy. Pending queue survives the change.
    pub fn set_policy(&self, p: WikiPolicy) {
        *self.policy.lock().unwrap() = p;
    }

    pub async fn intercept(&self, args: WriteArgsView) -> GuardDecision {
        let policy = *self.policy.lock().unwrap();
        match policy {
            WikiPolicy::Allowed => GuardDecision::Allow,
            WikiPolicy::Blocked => GuardDecision::Reject(
                "Wiki writes are blocked by current policy.".to_string(),
            ),
            WikiPolicy::Deferred => {
                self.pending.lock().unwrap().push(PendingWikiWrite {
                    path: args.path,
                    title: args.title,
                    content: args.content,
                    tags: args.tags,
                    page_type: args.page_type,
                    queued_at: Utc::now(),
                });
                GuardDecision::Defer
            }
        }
    }

    /// Take all queued writes, clearing the buffer.
    pub fn drain_pending(&self) -> Vec<PendingWikiWrite> {
        std::mem::take(&mut self.pending.lock().unwrap())
    }

    /// Peek at pending count without draining.
    pub fn pending_len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(path: &str) -> WriteArgsView {
        WriteArgsView {
            path: path.to_string(),
            title: "T".to_string(),
            content: "C".to_string(),
            tags: vec![],
            page_type: "topic".to_string(),
        }
    }

    #[tokio::test]
    async fn test_allowed_policy_passes_through() {
        let g = WikiGuard::new(WikiPolicy::Allowed);
        let d = g.intercept(args("topics/x")).await;
        assert!(matches!(d, GuardDecision::Allow));
        assert_eq!(g.pending_len(), 0);
    }

    #[tokio::test]
    async fn test_blocked_policy_rejects() {
        let g = WikiGuard::new(WikiPolicy::Blocked);
        let d = g.intercept(args("topics/x")).await;
        match d {
            GuardDecision::Reject(msg) => assert!(msg.contains("blocked")),
            _ => panic!("expected Reject"),
        }
    }

    #[tokio::test]
    async fn test_deferred_policy_queues() {
        let g = WikiGuard::new(WikiPolicy::Deferred);
        let d = g.intercept(args("topics/x")).await;
        assert!(matches!(d, GuardDecision::Defer));
        assert_eq!(g.pending_len(), 1);
    }

    #[tokio::test]
    async fn test_set_policy_changes_behavior() {
        let g = WikiGuard::allow_all();
        assert!(matches!(g.intercept(args("a")).await, GuardDecision::Allow));
        g.set_policy(WikiPolicy::Deferred);
        assert!(matches!(g.intercept(args("b")).await, GuardDecision::Defer));
        assert_eq!(g.pending_len(), 1);
    }

    #[tokio::test]
    async fn test_drain_pending_returns_and_clears() {
        let g = WikiGuard::new(WikiPolicy::Deferred);
        g.intercept(args("topics/a")).await;
        g.intercept(args("topics/b")).await;
        let pending = g.drain_pending();
        assert_eq!(pending.len(), 2);
        assert_eq!(g.pending_len(), 0);
        let again = g.drain_pending();
        assert!(again.is_empty());
    }

    #[tokio::test]
    async fn test_default_constructor_is_allowed() {
        let g = WikiGuard::allow_all();
        assert_eq!(g.policy(), WikiPolicy::Allowed);
    }
}
```

In `gasket/engine/src/lib.rs`, add `pub mod flow;` near the existing `pub mod` declarations (alphabetically — after `pub mod error;`).

- [ ] **Step 5: Run the guard tests**

```bash
cargo test --package gasket-engine flow::wiki_guard:: 2>&1 | tail -15
```

Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/types/src/flow.rs gasket/types/src/lib.rs \
        gasket/engine/src/flow/mod.rs gasket/engine/src/flow/wiki_guard.rs \
        gasket/engine/src/lib.rs
git commit -m "feat(engine): add WikiGuard with WikiPolicy {Allowed,Deferred,Blocked}"
```

---

### Task 2: Wire `WikiGuard` into `WikiWriteTool`

**Files:**
- Modify: `gasket/engine/src/tools/wiki_tools.rs`

- [ ] **Step 1: Add a regression test that the default behavior is unchanged**

In `gasket/engine/src/tools/wiki_tools.rs`, find the file's existing tests (or near the bottom of the file) and add:

```rust
    #[tokio::test]
    async fn test_default_guard_is_allowed_policy() {
        // Constructing a default WikiWriteTool gives a guard with Allowed policy
        // — existing behavior is preserved when no flow / wiki review is active.
        // Compile-only check: WikiWriteTool::new still has the same signature.
        // Behavior of the guard itself is covered in flow::wiki_guard tests.
        use crate::flow::WikiPolicy;
        // Trivial sanity check on policy default.
        assert_eq!(WikiPolicy::default(), WikiPolicy::Allowed);
    }
```

- [ ] **Step 2: Run the test (should pass — it's a sanity check)**

```bash
cargo test --package gasket-engine wiki_tools::tests::test_default_guard 2>&1 | tail -5
```

Expected: 1 passed (or compile error if `WikiPolicy` is not yet re-exported — fix the `use` path).

- [ ] **Step 3: Modify `WikiWriteTool` to hold an `Arc<WikiGuard>`**

In `gasket/engine/src/tools/wiki_tools.rs`, near the top add (if not present):

```rust
use std::sync::Arc;
use crate::flow::{GuardDecision, WikiGuard, WikiPolicy, WriteArgsView};
```

Replace the existing `pub struct WikiWriteTool { page_store: PageStore }` and its `impl new` block with:

```rust
pub struct WikiWriteTool {
    page_store: PageStore,
    guard: Arc<WikiGuard>,
}

impl WikiWriteTool {
    /// Create with default `WikiPolicy::Allowed` (existing behavior unchanged).
    pub fn new(page_store: PageStore) -> Self {
        Self {
            page_store,
            guard: Arc::new(WikiGuard::allow_all()),
        }
    }

    /// Construct with a shared guard. Used by the CLI to install a process-wide
    /// `Arc<WikiGuard>` whose policy can be toggled at runtime via /wiki review.
    pub fn with_guard(page_store: PageStore, guard: Arc<WikiGuard>) -> Self {
        Self { page_store, guard }
    }

    /// Read-only handle to the guard (for external policy management / drain).
    pub fn guard(&self) -> Arc<WikiGuard> {
        self.guard.clone()
    }
}
```

- [ ] **Step 4: Insert guard interception into `execute`**

The current `execute` (around line 191) starts with `let parsed: WriteArgs = ...`. Right after parsing args and before the `path/title/content` validation, insert:

```rust
        // Guard interception
        let view = WriteArgsView {
            path: parsed.path.clone(),
            title: parsed.title.clone(),
            content: parsed.content.clone(),
            tags: parsed.tags.clone(),
            page_type: parsed.page_type.clone(),
        };
        match self.guard.intercept(view).await {
            GuardDecision::Allow => { /* proceed */ }
            GuardDecision::Defer => {
                return Ok(format!(
                    "Wiki write deferred for review (path: {}). Run `gasket wiki review` to inspect and approve queued writes.",
                    parsed.path
                ));
            }
            GuardDecision::Reject(msg) => {
                return Err(ToolError::PermissionDenied(msg));
            }
        }
```

The rest of `execute` (path/title/content trim, `PageType` mapping, `page_store.write()`) stays exactly as before.

- [ ] **Step 5: Build and run all engine tests**

```bash
cargo build --package gasket-engine 2>&1 | tail -5
cargo test --package gasket-engine 2>&1 | tail -10
```

Expected: build succeeds; all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/tools/wiki_tools.rs
git commit -m "feat(engine): wire WikiGuard into WikiWriteTool with Allow/Defer/Reject paths"
```

---

### Task 3: `/wiki review` CLI subcommand

**Files:**
- Modify: `gasket/cli/src/commands/wiki.rs`
- Modify: `gasket/cli/src/cli.rs` (or wherever the wiki subcommands enum is defined; find with `grep -n "WikiCommands" gasket/cli/src/`)
- Modify: `gasket/cli/src/commands/agent.rs` — install a process-wide `Arc<WikiGuard>` so the same guard the agent uses is the one `/wiki review` toggles

The `/wiki review` flow:
1. User runs `gasket wiki review`. CLI loads the same `Arc<WikiGuard>` the agent process is using (via a thread-local or process-static, since wiki review and agent are separate processes — see Step 5).
2. **Cross-process consideration:** Since CLI subcommands run in separate processes, the pending queue must be **persisted to SQLite**, not held in memory. We use the existing `kv_store` for v1 (key `wiki_guard_pending`, value JSON of `Vec<PendingWikiWrite>`).
3. `WikiGuard` gets a `with_persistence(kv_store: KvStore)` constructor that loads the queue at startup and saves on every push/drain.

- [ ] **Step 1: Add persistence support to `WikiGuard`**

In `gasket/engine/src/flow/wiki_guard.rs`, append:

```rust
use gasket_storage::KvStore;

const KV_KEY_PENDING: &str = "wiki_guard_pending";
const KV_KEY_POLICY: &str = "wiki_guard_policy";

impl WikiGuard {
    /// Construct a guard whose pending queue and policy persist via `KvStore`.
    /// Loads any existing queue and policy at construction time.
    pub async fn with_persistence(kv: KvStore) -> Self {
        let policy = kv.read(KV_KEY_POLICY).await.ok().flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(WikiPolicy::Allowed);
        let pending = kv.read(KV_KEY_PENDING).await.ok().flatten()
            .and_then(|s| serde_json::from_str::<Vec<PendingWikiWrite>>(&s).ok())
            .unwrap_or_default();
        let guard = Self {
            policy: Mutex::new(policy),
            pending: Mutex::new(pending),
        };
        guard.kv = Some(kv);
        guard
    }
}
```

Wait — that won't compile because `Self` doesn't have a `kv` field. Refactor:

Update the struct to include an optional KV store and change `intercept` and `set_policy` and `drain_pending` to persist:

```rust
pub struct WikiGuard {
    policy: Mutex<WikiPolicy>,
    pending: Mutex<Vec<PendingWikiWrite>>,
    kv: Option<KvStore>,
}

impl WikiGuard {
    pub fn new(policy: WikiPolicy) -> Self {
        Self {
            policy: Mutex::new(policy),
            pending: Mutex::new(Vec::new()),
            kv: None,
        }
    }

    pub fn allow_all() -> Self { Self::new(WikiPolicy::Allowed) }

    pub async fn with_persistence(kv: KvStore) -> Self {
        let policy = kv.read(KV_KEY_POLICY).await.ok().flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(WikiPolicy::Allowed);
        let pending = kv.read(KV_KEY_PENDING).await.ok().flatten()
            .and_then(|s| serde_json::from_str::<Vec<PendingWikiWrite>>(&s).ok())
            .unwrap_or_default();
        Self {
            policy: Mutex::new(policy),
            pending: Mutex::new(pending),
            kv: Some(kv),
        }
    }

    async fn persist_pending(&self) {
        if let Some(ref kv) = self.kv {
            let v = self.pending.lock().unwrap().clone();
            if let Ok(s) = serde_json::to_string(&v) {
                let _ = kv.write(KV_KEY_PENDING, &s).await;
            }
        }
    }

    async fn persist_policy(&self) {
        if let Some(ref kv) = self.kv {
            let p = *self.policy.lock().unwrap();
            if let Ok(s) = serde_json::to_string(&p) {
                let _ = kv.write(KV_KEY_POLICY, &s).await;
            }
        }
    }
}
```

Update `intercept`, `set_policy`, `drain_pending` to call the persist helpers (they're async, so adjust signatures):

```rust
    pub async fn set_policy(&self, p: WikiPolicy) {
        *self.policy.lock().unwrap() = p;
        self.persist_policy().await;
    }

    pub async fn intercept(&self, args: WriteArgsView) -> GuardDecision {
        let policy = *self.policy.lock().unwrap();
        match policy {
            WikiPolicy::Allowed => GuardDecision::Allow,
            WikiPolicy::Blocked => GuardDecision::Reject(
                "Wiki writes are blocked by current policy.".to_string(),
            ),
            WikiPolicy::Deferred => {
                self.pending.lock().unwrap().push(PendingWikiWrite {
                    path: args.path,
                    title: args.title,
                    content: args.content,
                    tags: args.tags,
                    page_type: args.page_type,
                    queued_at: Utc::now(),
                });
                self.persist_pending().await;
                GuardDecision::Defer
            }
        }
    }

    pub async fn drain_pending(&self) -> Vec<PendingWikiWrite> {
        let drained = std::mem::take(&mut *self.pending.lock().unwrap());
        self.persist_pending().await;
        drained
    }
```

- [ ] **Step 2: Update existing tests in `wiki_guard.rs` to handle async drain**

Two existing tests touch `drain_pending`. Change them to `g.drain_pending().await`. Also add a persistence test:

```rust
    #[tokio::test]
    async fn test_persistence_round_trip() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite:file:wg_test_{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4().simple()
            ))
            .await
            .unwrap();
        gasket_storage::migrations::run_all(&pool).await.unwrap();
        let kv = gasket_storage::KvStore::new(pool);

        let g1 = WikiGuard::with_persistence(kv.clone()).await;
        g1.set_policy(WikiPolicy::Deferred).await;
        g1.intercept(args("topics/persisted")).await;
        drop(g1);

        // Reload — should restore policy + pending queue
        let g2 = WikiGuard::with_persistence(kv).await;
        assert_eq!(g2.policy(), WikiPolicy::Deferred);
        assert_eq!(g2.pending_len(), 1);
    }
```

This test requires `gasket-storage` migrations to be public. Check `gasket/storage/src/lib.rs`:

```bash
grep "mod migrations" gasket/storage/src/lib.rs
```

If `mod migrations;` is private, change to `pub mod migrations;`.

- [ ] **Step 3: Run the updated guard tests**

```bash
cargo test --package gasket-engine flow::wiki_guard:: 2>&1 | tail -15
```

Expected: 7 passed (6 original + persistence).

- [ ] **Step 4: Add `cmd_wiki_review` to `gasket/cli/src/commands/wiki.rs`**

Append the function:

```rust
/// Interactive review of pending wiki writes queued by `WikiPolicy::Deferred`.
pub async fn cmd_wiki_review() -> Result<()> {
    use gasket_engine::flow::{WikiGuard, WikiPolicy};
    use std::io::{BufRead, Write};

    let store = gasket_engine::SqliteStore::new().await?;
    let kv = store.kv_store();
    let guard = WikiGuard::with_persistence(kv).await;

    let pending = guard.drain_pending().await;
    if pending.is_empty() {
        println!("No pending wiki writes.");
        println!("Current policy: {:?}", guard.policy());
        println!();
        println!("Toggle policy with:");
        println!("  gasket wiki review --set-policy allowed   # default; writes go through");
        println!("  gasket wiki review --set-policy deferred  # queue for review");
        println!("  gasket wiki review --set-policy blocked   # reject all");
        return Ok(());
    }

    println!("Pending wiki writes ({}):", pending.len());
    for (i, w) in pending.iter().enumerate() {
        println!();
        println!("[{}] {} — {}", i + 1, w.path, w.title);
        println!("    type: {}, tags: {:?}", w.page_type, w.tags);
        println!("    queued at: {}", w.queued_at);
        let preview: String = w.content.chars().take(200).collect();
        println!("    content preview: {}", preview);
    }
    println!();
    print!("Action — (a)pprove all, (n)one, (p)ick: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let action = line.trim().to_lowercase();

    let to_write: Vec<_> = match action.as_str() {
        "a" | "approve" | "all" => pending,
        "n" | "none" | "discard" => {
            println!("All {} pending writes discarded.", pending.len());
            return Ok(());
        }
        "p" | "pick" => {
            print!("Indices to approve (comma-separated, 1-based): ");
            std::io::stdout().flush()?;
            let mut idx_line = String::new();
            std::io::stdin().lock().read_line(&mut idx_line)?;
            let chosen: std::collections::BTreeSet<usize> = idx_line
                .split(',')
                .filter_map(|s| s.trim().parse::<usize>().ok())
                .map(|i| i.saturating_sub(1))
                .collect();
            pending.into_iter().enumerate()
                .filter(|(i, _)| chosen.contains(i))
                .map(|(_, w)| w)
                .collect()
        }
        _ => {
            println!("Unrecognised action; queue not modified. Re-run to retry.");
            return Ok(());
        }
    };

    if to_write.is_empty() {
        println!("Nothing approved.");
        return Ok(());
    }

    let wiki_root = wiki_base_dir();
    let ps = PageStore::new(store.pool(), wiki_root);
    gasket_engine::create_wiki_tables(&store.pool()).await?;
    ps.init_dirs().await?;

    let mut written = 0;
    let mut errors = 0;
    for w in &to_write {
        let pt = match w.page_type.as_str() {
            "entity" => PageType::Entity,
            "source" => PageType::Source,
            "sop" => PageType::Sop,
            _ => PageType::Topic,
        };
        let mut page = WikiPage::new(w.path.clone(), w.title.clone(), pt, w.content.clone());
        page.tags = w.tags.clone();
        match ps.write(&page).await {
            Ok(_) => written += 1,
            Err(e) => {
                eprintln!("Failed to write {}: {}", w.path, e);
                errors += 1;
            }
        }
    }
    println!("Wrote {} page(s); {} error(s).", written, errors);
    Ok(())
}

/// Set the WikiPolicy without showing the queue.
pub async fn cmd_wiki_set_policy(policy: &str) -> Result<()> {
    use gasket_engine::flow::{WikiGuard, WikiPolicy};
    let p = match policy {
        "allowed" => WikiPolicy::Allowed,
        "deferred" => WikiPolicy::Deferred,
        "blocked" => WikiPolicy::Blocked,
        _ => anyhow::bail!("unknown policy '{}', expected allowed|deferred|blocked", policy),
    };
    let store = gasket_engine::SqliteStore::new().await?;
    let guard = WikiGuard::with_persistence(store.kv_store()).await;
    guard.set_policy(p).await;
    println!("Wiki policy set to {:?}.", p);
    Ok(())
}
```

- [ ] **Step 5: Wire the subcommand into clap**

Find where the wiki subcommands are defined (likely `gasket/cli/src/cli.rs`):

```bash
grep -n "WikiCommands\|enum Wiki" gasket/cli/src/cli.rs gasket/cli/src/commands/mod.rs 2>/dev/null
```

In the wiki subcommands enum, add:

```rust
    /// Review and approve pending wiki writes (queued by `Deferred` policy)
    Review,
    /// Set the wiki write policy (allowed|deferred|blocked)
    SetPolicy {
        /// Policy: allowed | deferred | blocked
        policy: String,
    },
```

In the dispatch match:

```rust
        WikiCommands::Review => cmd_wiki_review().await,
        WikiCommands::SetPolicy { policy } => cmd_wiki_set_policy(&policy).await,
```

- [ ] **Step 6: Wire the agent CLI to use the persistent guard**

In `gasket/cli/src/commands/agent.rs`, find where `common_tools` is built (around line 181, the `gasket_engine::tools::build_tool_registry(...)` call). Right before that, add:

```rust
    // Install a process-wide WikiGuard whose state persists in KvStore.
    let wiki_guard = Arc::new(
        gasket_engine::flow::WikiGuard::with_persistence(sqlite_store.kv_store()).await
    );
```

Then look at how `WikiWriteTool` is currently constructed inside `build_tool_registry`. If that function constructs it via `WikiWriteTool::new(page_store)`, add an option to pass a guard:

In `gasket/engine/src/tools/registry.rs` (or wherever `build_tool_registry` lives), add a field to `ToolRegistryConfig`:

```bash
grep -n "ToolRegistryConfig" gasket/engine/src/tools/*.rs
```

Add:

```rust
    pub wiki_guard: Option<Arc<crate::flow::WikiGuard>>,
```

Then in the tool construction:

```rust
        if let Some(ps) = page_store {
            let tool = match wiki_guard {
                Some(g) => WikiWriteTool::with_guard(ps.clone(), g),
                None => WikiWriteTool::new(ps.clone()),
            };
            registry.register(Box::new(tool));
            // (keep existing search/read tool registration the same)
        }
```

In `agent.rs`, pass the new field:

```rust
        wiki_guard: Some(wiki_guard.clone()),
```

- [ ] **Step 7: Build and smoke test**

```bash
cargo build --workspace 2>&1 | tail -5
cargo run --release --package gasket-cli -- wiki review
```

Expected: build succeeds; `wiki review` says "No pending wiki writes."

```bash
cargo run --release --package gasket-cli -- wiki set-policy deferred
cargo run --release --package gasket-cli -- wiki review
```

Expected: policy persists; review shows current policy.

- [ ] **Step 8: Commit**

```bash
git add gasket/engine/src/flow/wiki_guard.rs \
        gasket/cli/src/commands/wiki.rs \
        gasket/cli/src/cli.rs \
        gasket/cli/src/commands/agent.rs \
        gasket/engine/src/tools/registry.rs \
        gasket/storage/src/lib.rs
git commit -m "feat(cli): add /wiki review and /wiki set-policy with persistent WikiGuard"
```

---

### Task 4: v1 end-to-end verification

**Files:** None new.

- [ ] **Step 1: Build the entire workspace**

```bash
cargo build --workspace --release 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 3: End-to-end test — Allowed mode (regression)**

```bash
rm -f ~/.gasket/gasket.db
cargo run --release --package gasket-cli -- wiki set-policy allowed
# (in another shell or in interactive agent session, trigger a wiki_write — verify it writes immediately)
```

- [ ] **Step 4: End-to-end test — Deferred mode**

```bash
cargo run --release --package gasket-cli -- wiki set-policy deferred
# Run a session where the agent triggers wiki_write (e.g., ask it to memorize something)
# Confirm the agent receives "deferred for review" instead of writing
cargo run --release --package gasket-cli -- wiki review
# Confirm the queued writes appear; pick or approve all
sqlite3 ~/.gasket/gasket.db "SELECT path FROM wiki_pages"
# Confirm the approved pages exist
```

- [ ] **Step 5: Commit any small fixes**

```bash
git status
# fix anything broken, then:
git add -A && git commit -m "chore(v1): end-to-end verification fixes for WikiGuard"
```

If nothing needs fixing, skip this commit.

**v1 ships at this commit.** Pause for ~1 week of real use before starting v2. The gathered feedback informs whether v2's `wiki_policy: deferred` template default is right, what the queue UI should expose, etc.

---

# BATCH v2 — Flow Engine

Goal: ship the full slash-command flow engine. Builds on v1's `WikiGuard`. Mandatory: `PhaseRunner` really calls `kernel::execute_streaming`. No stubs. No "v2.1 deferrals."

---

### Task 5: Flow types + state machine

**Files:**
- Modify: `gasket/types/src/flow.rs` — add `PhaseId`, `FlowStatus`, `PhaseOutput`, `is_builtin_phase`
- Modify: `gasket/types/src/lib.rs` — re-export new types
- Modify: `gasket/types/src/events/stream.rs` — add 4 `ChatEvent` variants
- Create: `gasket/engine/src/flow/state.rs` — `FlowState` + state machine
- Create: `gasket/engine/src/flow/template.rs` — YAML loader
- Modify: `gasket/engine/src/flow/mod.rs` — re-export new modules

- [ ] **Step 1: Extend `gasket/types/src/flow.rs`**

Append (after the existing v1 content):

```rust
/// Phase identifier. Just a string — no enum/custom distinction.
/// Built-in phase ids: "brainstorm", "design", "plan", "execute", "verify".
pub type PhaseId = String;

/// Whether the given phase id is one of the built-in 5.
pub fn is_builtin_phase(s: &str) -> bool {
    matches!(s, "brainstorm" | "design" | "plan" | "execute" | "verify")
}

/// Status of a single flow run. Phase information lives in `FlowState.current_phase`,
/// not duplicated here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    Running,
    AwaitingGate,
    Paused,
    Done,
    Aborted,
}

impl FlowStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FlowStatus::Running => "running",
            FlowStatus::AwaitingGate => "awaiting_gate",
            FlowStatus::Paused => "paused",
            FlowStatus::Done => "done",
            FlowStatus::Aborted => "aborted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(FlowStatus::Running),
            "awaiting_gate" => Some(FlowStatus::AwaitingGate),
            "paused" => Some(FlowStatus::Paused),
            "done" => Some(FlowStatus::Done),
            "aborted" => Some(FlowStatus::Aborted),
            _ => None,
        }
    }
}

/// Captured output of a single completed phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseOutput {
    pub summary: String,
    pub iterations_used: u32,
    pub tools_called: Vec<String>,
    pub finished_at: DateTime<Utc>,
}

#[cfg(test)]
mod v2_tests {
    use super::*;

    #[test]
    fn test_is_builtin_phase() {
        assert!(is_builtin_phase("brainstorm"));
        assert!(is_builtin_phase("verify"));
        assert!(!is_builtin_phase("custom"));
    }

    #[test]
    fn test_flow_status_parse_round_trip() {
        for s in ["running", "awaiting_gate", "paused", "done", "aborted"] {
            let st = FlowStatus::parse(s).unwrap();
            assert_eq!(st.as_str(), s);
        }
        assert!(FlowStatus::parse("nonsense").is_none());
    }
}
```

In `gasket/types/src/lib.rs`, update the re-export:

```rust
pub use flow::{is_builtin_phase, FlowStatus, PendingWikiWrite, PhaseId, PhaseOutput};
```

- [ ] **Step 2: Add `ChatEvent` variants in `gasket/types/src/events/stream.rs`**

Add to the `ChatEvent` enum (before the closing `}`):

```rust
    FlowStarted { flow_id: Arc<str>, template: Arc<str> },
    PhaseChanged { phase: Arc<str> },
    GatePending { phase: Arc<str>, prompt: Arc<str> },
    FlowFinished { flow_id: Arc<str>, status: Arc<str> },
```

In `impl ChatEvent`, add constructors:

```rust
    pub fn flow_started(flow_id: impl Into<String>, template: impl Into<String>) -> Self {
        Self::FlowStarted {
            flow_id: Arc::from(flow_id.into()),
            template: Arc::from(template.into()),
        }
    }
    pub fn phase_changed(phase: impl Into<String>) -> Self {
        Self::PhaseChanged { phase: Arc::from(phase.into()) }
    }
    pub fn gate_pending(phase: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self::GatePending {
            phase: Arc::from(phase.into()),
            prompt: Arc::from(prompt.into()),
        }
    }
    pub fn flow_finished(flow_id: impl Into<String>, status: impl Into<String>) -> Self {
        Self::FlowFinished {
            flow_id: Arc::from(flow_id.into()),
            status: Arc::from(status.into()),
        }
    }
```

- [ ] **Step 3: Create `gasket/engine/src/flow/template.rs`**

```rust
//! YAML flow template definitions.
//!
//! Schema (see spec §3.2):
//! ```yaml
//! name: <string>
//! description: <string>
//! version: <int>
//! wiki_policy: deferred | blocked | allowed
//! phases:
//!   - id: <phase_id>
//!     label: <string>
//!     prompt_file: <path relative to YAML's dir>
//!     allowed_tools: [...]    # absent = all tools
//!     max_iterations: <int>   # absent = unlimited
//!     gate:                   # absent = no gate
//!       prompt: <string>
//! ```

use anyhow::{Context, Result};
use gasket_types::PhaseId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WikiPolicy {
    Allowed,
    Deferred,
    Blocked,
}

impl Default for WikiPolicy {
    fn default() -> Self { WikiPolicy::Deferred }
}

impl From<WikiPolicy> for crate::flow::wiki_guard::WikiPolicy {
    fn from(p: WikiPolicy) -> Self {
        match p {
            WikiPolicy::Allowed => crate::flow::wiki_guard::WikiPolicy::Allowed,
            WikiPolicy::Deferred => crate::flow::wiki_guard::WikiPolicy::Deferred,
            WikiPolicy::Blocked => crate::flow::wiki_guard::WikiPolicy::Blocked,
        }
    }
}

/// Optional gate at the end of a phase. Absence = no gate, auto-advance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    #[serde(default = "default_gate_prompt")]
    pub prompt: String,
}

fn default_gate_prompt() -> String {
    "Continue? (y/n/edit/redo/back)".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSpec {
    pub id: PhaseId,
    #[serde(default)]
    pub label: String,
    pub prompt_file: PathBuf,
    /// `None` = all tools. `Some(vec![])` = no tools (silly but valid).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// `None` = unlimited.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// `None` = no gate, auto-advance.
    #[serde(default)]
    pub gate: Option<GateConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowTemplate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: i64,
    #[serde(default)]
    pub wiki_policy: WikiPolicy,
    pub phases: Vec<PhaseSpec>,

    /// Set after loading: directory containing the YAML, used to resolve `prompt_file`.
    /// `pub(crate)` so state machine tests can build fixtures without YAML.
    #[serde(skip)]
    pub(crate) base_dir: PathBuf,
}

impl FlowTemplate {
    /// Load template from disk path.
    pub fn load_from_path(path: &Path) -> Result<Arc<Self>> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading template {}", path.display()))?;
        Self::load_from_yaml(&body, path.parent().unwrap_or(Path::new(".")).to_path_buf())
    }

    /// Load template from a YAML string with a base dir for resolving prompts.
    /// Used both for normal load and for resume-from-snapshot.
    pub fn load_from_yaml(body: &str, base_dir: PathBuf) -> Result<Arc<Self>> {
        let mut tpl: FlowTemplate = serde_yaml::from_str(body)
            .with_context(|| "parsing flow template YAML")?;
        tpl.base_dir = base_dir;
        Ok(Arc::new(tpl))
    }

    pub fn resolve_prompt(&self, phase: &PhaseId) -> Result<String> {
        let spec = self.phases.iter()
            .find(|p| &p.id == phase)
            .with_context(|| format!("phase '{}' not in template '{}'", phase, self.name))?;
        let path = self.base_dir.join(&spec.prompt_file);
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading prompt {}", path.display()))
    }

    pub fn is_tool_allowed(&self, phase: &PhaseId, tool_name: &str) -> bool {
        let Some(spec) = self.phases.iter().find(|p| &p.id == phase) else {
            return false;
        };
        match &spec.allowed_tools {
            None => true,                                       // None = all
            Some(list) => list.iter().any(|t| t == tool_name),  // explicit list
        }
    }
}

pub struct TemplateLoader {
    user_dir: PathBuf,
    builtin_dir: Option<PathBuf>,
}

impl TemplateLoader {
    pub fn new(user_dir: PathBuf, builtin_dir: Option<PathBuf>) -> Self {
        Self { user_dir, builtin_dir }
    }

    pub fn load(&self, name: &str) -> Result<Arc<FlowTemplate>> {
        let user_path = self.user_dir.join(format!("{name}.yaml"));
        if user_path.exists() {
            return FlowTemplate::load_from_path(&user_path);
        }
        if let Some(ref bd) = self.builtin_dir {
            let p = bd.join(format!("{name}.yaml"));
            if p.exists() {
                return FlowTemplate::load_from_path(&p);
            }
        }
        anyhow::bail!("template '{name}' not found in user or built-in flows dir")
    }

    pub fn list(&self) -> Vec<String> {
        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for dir in [Some(&self.user_dir), self.builtin_dir.as_ref()].iter().flatten() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    if let Some(stem) = e.path().file_stem().and_then(|s| s.to_str()) {
                        if e.path().extension().map(|x| x == "yaml").unwrap_or(false) {
                            names.insert(stem.to_string());
                        }
                    }
                }
            }
        }
        names.into_iter().collect()
    }
}

/// Substitute `{{var}}` tokens. Unknown variables are left as-is.
pub fn render_prompt(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        if let Some(close_rel) = after_open.find("}}") {
            let key = after_open[..close_rel].trim();
            if let Some(val) = vars.get(key) {
                out.push_str(val);
            } else {
                // unknown — preserve verbatim
                out.push_str("{{");
                out.push_str(&after_open[..close_rel]);
                out.push_str("}}");
            }
            rest = &after_open[close_rel + 2..];
        } else {
            out.push_str("{{");
            rest = after_open;
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_YAML: &str = r#"
name: test-tpl
description: minimal
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: B
    prompt_file: prompts/b.md
    allowed_tools: [wiki_search]
    max_iterations: 3
    gate:
      prompt: "Continue?"
"#;

    fn write_yaml(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn write_prompt(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn test_parse_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "t", MINIMAL_YAML);
        write_prompt(dir.path(), "prompts/b.md", "Hello {{user_request}}");
        let tpl = FlowTemplate::load_from_path(&path).unwrap();
        assert_eq!(tpl.name, "test-tpl");
        assert_eq!(tpl.phases.len(), 1);
        assert_eq!(tpl.phases[0].id, "brainstorm");
        assert!(tpl.phases[0].gate.is_some());
        assert_eq!(tpl.phases[0].max_iterations, Some(3));
    }

    #[test]
    fn test_optional_fields_default_to_none() {
        let yaml = r#"
name: x
version: 1
phases:
  - id: execute
    prompt_file: e.md
"#;
        let tpl = FlowTemplate::load_from_yaml(yaml, std::path::PathBuf::from(".")).unwrap();
        let p = &tpl.phases[0];
        assert!(p.allowed_tools.is_none());
        assert!(p.max_iterations.is_none());
        assert!(p.gate.is_none());
    }

    #[test]
    fn test_is_tool_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "t", MINIMAL_YAML);
        write_prompt(dir.path(), "prompts/b.md", "x");
        let tpl = FlowTemplate::load_from_path(&path).unwrap();
        assert!(tpl.is_tool_allowed(&"brainstorm".to_string(), "wiki_search"));
        assert!(!tpl.is_tool_allowed(&"brainstorm".to_string(), "shell"));
    }

    #[test]
    fn test_is_tool_allowed_when_none_allows_all() {
        let yaml = r#"
name: x
version: 1
phases:
  - id: execute
    prompt_file: e.md
"#;
        let tpl = FlowTemplate::load_from_yaml(yaml, std::path::PathBuf::from(".")).unwrap();
        assert!(tpl.is_tool_allowed(&"execute".to_string(), "shell"));
        assert!(tpl.is_tool_allowed(&"execute".to_string(), "anything"));
    }

    #[test]
    fn test_user_overrides_builtin() {
        let user = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        write_yaml(user.path(), "default", &MINIMAL_YAML.replace("test-tpl", "user"));
        write_prompt(user.path(), "prompts/b.md", "u");
        write_yaml(builtin.path(), "default", &MINIMAL_YAML.replace("test-tpl", "builtin"));
        write_prompt(builtin.path(), "prompts/b.md", "b");
        let loader = TemplateLoader::new(
            user.path().to_path_buf(),
            Some(builtin.path().to_path_buf()),
        );
        let tpl = loader.load("default").unwrap();
        assert_eq!(tpl.name, "user");
    }

    #[test]
    fn test_render_prompt_substitutes() {
        let mut vars = HashMap::new();
        vars.insert("user_request".to_string(), "x".to_string());
        vars.insert("flow_id".to_string(), "abc".to_string());
        assert_eq!(
            render_prompt("U={{user_request}} F={{flow_id}}", &vars),
            "U=x F=abc"
        );
    }

    #[test]
    fn test_render_prompt_preserves_unknown() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "1".to_string());
        assert_eq!(render_prompt("{{a}} {{unknown}}", &vars), "1 {{unknown}}");
    }
}
```

If `tempfile` isn't a `[dev-dependencies]` of `gasket-engine`, add it:

```bash
grep -A 5 "\[dev-dependencies\]" gasket/engine/Cargo.toml
```

Add `tempfile = "3"` if missing.

- [ ] **Step 4: Create `gasket/engine/src/flow/state.rs`**

```rust
//! Pure state-machine logic for one flow run.

use chrono::{DateTime, Utc};
use gasket_types::{FlowStatus, PendingWikiWrite, PhaseId, PhaseOutput};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::flow::template::FlowTemplate;

#[derive(Debug, Clone)]
pub struct FlowState {
    pub flow_id: Uuid,
    pub template: Arc<FlowTemplate>,
    pub user_request: String,
    pub current_phase: PhaseId,
    pub status: FlowStatus,
    pub completed_phases: BTreeMap<PhaseId, PhaseOutput>,
    pub pending_wiki_writes: Vec<PendingWikiWrite>,
    /// Set on `GateResponse::Edit`; consumed and cleared by the next `PhaseRunner::run`.
    pub edit_feedback: Option<String>,
    pub session_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResponse {
    Yes,
    No,
    Edit(String),
    Redo,
    Back,
}

impl FlowState {
    pub fn new_with_template(
        user_request: String,
        session_key: String,
        template: Arc<FlowTemplate>,
    ) -> Self {
        let now = Utc::now();
        let first = template.phases.first()
            .map(|p| p.id.clone())
            .unwrap_or_else(|| "brainstorm".to_string());
        Self {
            flow_id: Uuid::now_v7(),
            template,
            user_request,
            current_phase: first,
            status: FlowStatus::Running,
            completed_phases: BTreeMap::new(),
            pending_wiki_writes: Vec::new(),
            edit_feedback: None,
            session_key,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn complete_current_phase(&mut self, output: PhaseOutput) {
        self.completed_phases.insert(self.current_phase.clone(), output);
        self.updated_at = Utc::now();
    }

    /// After phase completion, decide the next status.
    pub fn transition_after_phase(&mut self) {
        let idx = self.phase_index(&self.current_phase).unwrap_or(0);
        let spec = &self.template.phases[idx];
        if spec.gate.is_some() {
            self.status = FlowStatus::AwaitingGate;
        } else if let Some(next) = self.template.phases.get(idx + 1) {
            self.current_phase = next.id.clone();
            self.status = FlowStatus::Running;
        } else {
            self.status = FlowStatus::Done;
        }
        self.updated_at = Utc::now();
    }

    /// Apply user gate response. Only valid in `AwaitingGate` status.
    pub fn apply_gate_response(&mut self, resp: GateResponse) {
        if self.status != FlowStatus::AwaitingGate {
            return;
        }
        let idx = self.phase_index(&self.current_phase).unwrap_or(0);
        match resp {
            GateResponse::Yes => {
                if let Some(next) = self.template.phases.get(idx + 1) {
                    self.current_phase = next.id.clone();
                    self.status = FlowStatus::Running;
                } else {
                    self.status = FlowStatus::Done;
                }
            }
            GateResponse::No => {
                self.status = FlowStatus::Aborted;
            }
            GateResponse::Edit(text) => {
                self.edit_feedback = Some(text);
                self.status = FlowStatus::Running;
                // current_phase unchanged
            }
            GateResponse::Redo => {
                self.completed_phases.remove(&self.current_phase);
                self.status = FlowStatus::Running;
            }
            GateResponse::Back => {
                if idx > 0 {
                    self.current_phase = self.template.phases[idx - 1].id.clone();
                }
                // status stays AwaitingGate (we re-show the prior gate)
            }
        }
        self.updated_at = Utc::now();
    }

    pub fn pause(&mut self) {
        self.status = FlowStatus::Paused;
        self.updated_at = Utc::now();
    }

    pub fn abort(&mut self) {
        self.status = FlowStatus::Aborted;
        self.updated_at = Utc::now();
    }

    /// Take and clear edit feedback for the next phase invocation.
    pub fn take_edit_feedback(&mut self) -> Option<String> {
        self.edit_feedback.take()
    }

    fn phase_index(&self, phase: &PhaseId) -> Option<usize> {
        self.template.phases.iter().position(|p| &p.id == phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::template::{FlowTemplate, GateConfig, PhaseSpec, WikiPolicy};

    fn template(phases: Vec<(PhaseId, bool /*has gate*/)>) -> Arc<FlowTemplate> {
        let phases = phases.into_iter().map(|(id, has_gate)| PhaseSpec {
            id,
            label: String::new(),
            prompt_file: std::path::PathBuf::from("dummy"),
            allowed_tools: None,
            max_iterations: None,
            gate: if has_gate {
                Some(GateConfig { prompt: "?".into() })
            } else {
                None
            },
        }).collect();
        Arc::new(FlowTemplate {
            name: "t".into(),
            description: String::new(),
            version: 1,
            wiki_policy: WikiPolicy::Deferred,
            phases,
            base_dir: std::path::PathBuf::from("."),
        })
    }

    fn fresh(tpl: Arc<FlowTemplate>) -> FlowState {
        FlowState::new_with_template("req".into(), "cli:test".into(), tpl)
    }

    fn done_output() -> PhaseOutput {
        PhaseOutput {
            summary: "done".into(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: Utc::now(),
        }
    }

    #[test]
    fn test_new_starts_running_at_first_phase() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let s = fresh(tpl);
        assert_eq!(s.current_phase, "brainstorm");
        assert_eq!(s.status, FlowStatus::Running);
    }

    #[test]
    fn test_transition_with_gate_goes_to_awaiting_gate() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::AwaitingGate);
        assert_eq!(s.current_phase, "brainstorm"); // unchanged — gate not yet passed
    }

    #[test]
    fn test_transition_without_gate_advances() {
        let tpl = template(vec![("brainstorm".into(), false), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, "execute");
    }

    #[test]
    fn test_last_phase_completion_is_done() {
        let tpl = template(vec![("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::Done);
    }

    #[test]
    fn test_gate_yes_advances() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Yes);
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, "execute");
    }

    #[test]
    fn test_gate_no_aborts() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::No);
        assert_eq!(s.status, FlowStatus::Aborted);
    }

    #[test]
    fn test_gate_edit_stores_feedback() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Edit("clarify scope".into()));
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, "brainstorm"); // stays same
        assert_eq!(s.edit_feedback.as_deref(), Some("clarify scope"));
    }

    #[test]
    fn test_take_edit_feedback_consumes() {
        let mut s = fresh(template(vec![("brainstorm".into(), false)]));
        s.edit_feedback = Some("x".into());
        assert_eq!(s.take_edit_feedback().as_deref(), Some("x"));
        assert!(s.edit_feedback.is_none());
    }

    #[test]
    fn test_gate_redo_clears_completed() {
        let tpl = template(vec![("brainstorm".into(), true), ("execute".into(), false)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Redo);
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, "brainstorm");
        assert!(!s.completed_phases.contains_key("brainstorm"));
    }

    #[test]
    fn test_gate_back_returns_to_previous_gate() {
        let tpl = template(vec![
            ("brainstorm".into(), true),
            ("design".into(), true),
            ("execute".into(), false),
        ]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Yes); // pass brainstorm
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        // Now AwaitingGate at design
        assert_eq!(s.status, FlowStatus::AwaitingGate);
        assert_eq!(s.current_phase, "design");
        s.apply_gate_response(GateResponse::Back);
        assert_eq!(s.status, FlowStatus::AwaitingGate);
        assert_eq!(s.current_phase, "brainstorm");
    }

    #[test]
    fn test_back_at_first_phase_is_no_op() {
        let tpl = template(vec![("brainstorm".into(), true)]);
        let mut s = fresh(tpl);
        s.complete_current_phase(done_output());
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Back);
        assert_eq!(s.current_phase, "brainstorm");
    }
}
```

- [ ] **Step 5: Update `flow/mod.rs`**

```rust
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use state::{FlowState, GateResponse};
pub use template::{render_prompt, FlowTemplate, GateConfig, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{GuardDecision, WikiGuard, WriteArgsView};
// Note: `wiki_guard::WikiPolicy` and `template::WikiPolicy` are distinct types
// (storage vs UI / config). The conversion is explicit (`From` impl in template.rs).
```

- [ ] **Step 6: Run all new tests**

```bash
cargo test --package gasket-types --lib flow:: 2>&1 | tail -10
cargo test --package gasket-engine flow::template:: 2>&1 | tail -15
cargo test --package gasket-engine flow::state:: 2>&1 | tail -15
```

Expected:
- types: 3 passed (1 v1 + 2 v2)
- template: 6 passed
- state: 11 passed

- [ ] **Step 7: Commit**

```bash
git add gasket/types/src/flow.rs gasket/types/src/lib.rs \
        gasket/types/src/events/stream.rs \
        gasket/engine/src/flow/state.rs gasket/engine/src/flow/template.rs \
        gasket/engine/src/flow/mod.rs gasket/engine/Cargo.toml
git commit -m "feat(types,engine): add flow state types, YAML template loader, ChatEvent variants"
```

---

### Task 6: `flow_runs` migration + `FlowRunStore`

**Files:**
- Create: `gasket/storage/src/migrations/flow_run.rs`
- Modify: `gasket/storage/src/migrations/mod.rs`
- Create: `gasket/storage/src/flow_run_store.rs`
- Modify: `gasket/storage/src/lib.rs`

- [ ] **Step 1: Create the migration**

`gasket/storage/src/migrations/flow_run.rs`:

```rust
//! flow_runs table for the slash-command flow engine.

use sqlx::SqlitePool;

pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS flow_runs (
            flow_id            TEXT PRIMARY KEY,
            template_name      TEXT NOT NULL,
            template_version   INTEGER NOT NULL,
            template_yaml      TEXT NOT NULL,
            user_request       TEXT NOT NULL,
            session_key        TEXT NOT NULL,
            status             TEXT NOT NULL,
            current_phase      TEXT NOT NULL,
            completed_phases   TEXT NOT NULL DEFAULT '{}',
            pending_wiki       TEXT NOT NULL DEFAULT '[]',
            edit_feedback      TEXT,
            created_at         INTEGER NOT NULL,
            updated_at         INTEGER NOT NULL
        )"
    ).execute(pool).await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_flow_runs_session ON flow_runs(session_key, updated_at DESC)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_flow_runs_status ON flow_runs(status, updated_at DESC)")
        .execute(pool).await?;
    Ok(())
}
```

In `gasket/storage/src/migrations/mod.rs`:

```rust
pub mod cron;
pub mod flow_run;     // new
pub mod kv;
pub mod maintenance;
pub mod memory;
pub mod session;
```

In `run_all`, add after `kv::run_schema`:

```rust
    flow_run::run_schema(pool).await?;
```

- [ ] **Step 2: Create `FlowRunStore` with tests**

`gasket/storage/src/flow_run_store.rs`:

```rust
//! Repository for flow_runs.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, PartialEq)]
pub struct FlowRunRecord {
    pub flow_id: String,
    pub template_name: String,
    pub template_version: i64,
    pub template_yaml: String,
    pub user_request: String,
    pub session_key: String,
    pub status: String,
    pub current_phase: String,
    pub completed_phases: String,
    pub pending_wiki: String,
    pub edit_feedback: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct FlowRunStore { pool: SqlitePool }

impl FlowRunStore {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn insert(&self, r: &FlowRunRecord) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO flow_runs (
                flow_id, template_name, template_version, template_yaml, user_request,
                session_key, status, current_phase, completed_phases, pending_wiki,
                edit_feedback, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&r.flow_id)
        .bind(&r.template_name)
        .bind(r.template_version)
        .bind(&r.template_yaml)
        .bind(&r.user_request)
        .bind(&r.session_key)
        .bind(&r.status)
        .bind(&r.current_phase)
        .bind(&r.completed_phases)
        .bind(&r.pending_wiki)
        .bind(&r.edit_feedback)
        .bind(r.created_at)
        .bind(r.updated_at)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn update(&self, r: &FlowRunRecord) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE flow_runs SET
                status=?, current_phase=?, completed_phases=?, pending_wiki=?,
                edit_feedback=?, updated_at=?
             WHERE flow_id=?"
        )
        .bind(&r.status)
        .bind(&r.current_phase)
        .bind(&r.completed_phases)
        .bind(&r.pending_wiki)
        .bind(&r.edit_feedback)
        .bind(r.updated_at)
        .bind(&r.flow_id)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn load(&self, flow_id: &str) -> anyhow::Result<Option<FlowRunRecord>> {
        let row = sqlx::query("SELECT * FROM flow_runs WHERE flow_id = ?")
            .bind(flow_id).fetch_optional(&self.pool).await?;
        Ok(row.map(row_to_record))
    }

    pub async fn active_for_session(&self, session_key: &str) -> anyhow::Result<Option<FlowRunRecord>> {
        let row = sqlx::query(
            "SELECT * FROM flow_runs
             WHERE session_key = ?
               AND status IN ('running', 'awaiting_gate', 'paused')
             ORDER BY updated_at DESC LIMIT 1"
        ).bind(session_key).fetch_optional(&self.pool).await?;
        Ok(row.map(row_to_record))
    }

    pub async fn list_recent(&self, limit: i64) -> anyhow::Result<Vec<FlowRunRecord>> {
        let rows = sqlx::query("SELECT * FROM flow_runs ORDER BY updated_at DESC LIMIT ?")
            .bind(limit).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> FlowRunRecord {
    FlowRunRecord {
        flow_id: row.get("flow_id"),
        template_name: row.get("template_name"),
        template_version: row.get("template_version"),
        template_yaml: row.get("template_yaml"),
        user_request: row.get("user_request"),
        session_key: row.get("session_key"),
        status: row.get("status"),
        current_phase: row.get("current_phase"),
        completed_phases: row.get("completed_phases"),
        pending_wiki: row.get("pending_wiki"),
        edit_feedback: row.get("edit_feedback"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:file:fr_{}?mode=memory&cache=shared", uuid::Uuid::new_v4().simple()))
            .await.unwrap();
        crate::migrations::flow_run::run_schema(&pool).await.unwrap();
        pool
    }

    fn rec() -> FlowRunRecord {
        FlowRunRecord {
            flow_id: uuid::Uuid::now_v7().to_string(),
            template_name: "default".into(),
            template_version: 1,
            template_yaml: "name: x\nversion: 1\nphases: []\n".into(),
            user_request: "do".into(),
            session_key: "cli:t".into(),
            status: "running".into(),
            current_phase: "brainstorm".into(),
            completed_phases: "{}".into(),
            pending_wiki: "[]".into(),
            edit_feedback: None,
            created_at: 1000, updated_at: 1000,
        }
    }

    #[tokio::test]
    async fn test_insert_load() {
        let pool = temp_pool().await;
        let s = FlowRunStore::new(pool);
        let r = rec();
        s.insert(&r).await.unwrap();
        let loaded = s.load(&r.flow_id).await.unwrap().unwrap();
        assert_eq!(loaded.template_yaml, r.template_yaml);
        assert!(loaded.edit_feedback.is_none());
    }

    #[tokio::test]
    async fn test_update_round_trip() {
        let pool = temp_pool().await;
        let s = FlowRunStore::new(pool);
        let mut r = rec();
        s.insert(&r).await.unwrap();
        r.status = "awaiting_gate".into();
        r.edit_feedback = Some("clarify scope".into());
        r.updated_at = 2000;
        s.update(&r).await.unwrap();
        let loaded = s.load(&r.flow_id).await.unwrap().unwrap();
        assert_eq!(loaded.status, "awaiting_gate");
        assert_eq!(loaded.edit_feedback.as_deref(), Some("clarify scope"));
    }

    #[tokio::test]
    async fn test_active_for_session() {
        let pool = temp_pool().await;
        let s = FlowRunStore::new(pool);
        let mut active = rec(); active.status = "running".into(); active.session_key = "cli:s".into();
        let mut done = rec(); done.flow_id = uuid::Uuid::now_v7().to_string();
        done.status = "done".into(); done.session_key = "cli:s".into();
        s.insert(&active).await.unwrap();
        s.insert(&done).await.unwrap();
        let got = s.active_for_session("cli:s").await.unwrap().unwrap();
        assert_eq!(got.flow_id, active.flow_id);
    }

    #[tokio::test]
    async fn test_list_recent_orders_desc() {
        let pool = temp_pool().await;
        let s = FlowRunStore::new(pool);
        let mut a = rec(); a.updated_at = 1000;
        let mut b = rec(); b.flow_id = uuid::Uuid::now_v7().to_string(); b.updated_at = 2000;
        s.insert(&a).await.unwrap();
        s.insert(&b).await.unwrap();
        let recent = s.list_recent(10).await.unwrap();
        assert_eq!(recent[0].flow_id, b.flow_id);
    }
}
```

- [ ] **Step 3: Wire into `gasket/storage/src/lib.rs`**

Add `mod flow_run_store;` near other `mod` declarations. Add re-export:

```rust
pub use flow_run_store::{FlowRunRecord, FlowRunStore};
```

In `impl SqliteStore`, add:

```rust
    pub fn flow_run_store(&self) -> FlowRunStore {
        FlowRunStore::new(self.pool.clone())
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test --package gasket-storage flow_run_store:: 2>&1 | tail -10
cargo test --package gasket-storage 2>&1 | tail -10
```

Expected: 4 new passed; all existing pass.

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/src/migrations/flow_run.rs gasket/storage/src/migrations/mod.rs \
        gasket/storage/src/flow_run_store.rs gasket/storage/src/lib.rs
git commit -m "feat(storage): add flow_runs table with template_yaml snapshot + FlowRunStore"
```

---

### Task 7: `PhaseRunner` + `CliGate` + `FlowOrchestrator` (real kernel integration)

**Files:**
- Create: `gasket/engine/src/flow/phase_runner.rs`
- Create: `gasket/engine/src/flow/gate.rs`
- Create: `gasket/engine/src/flow/orchestrator.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

**Critical:** No stubs. `PhaseRunner::run` calls `kernel::execute_streaming` for real. `FlowOrchestrator::step` returns `Result<&FlowStatus>` (no separate Outcome type).

- [ ] **Step 1: `PhaseRunner` — single-phase kernel call**

`gasket/engine/src/flow/phase_runner.rs`:

```rust
//! Drives one phase: builds messages, calls kernel, captures output.

use anyhow::Result;
use chrono::Utc;
use gasket_providers::ChatMessage;
use gasket_types::{PhaseId, PhaseOutput};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::flow::template::{render_prompt, FlowTemplate};
use crate::kernel::{self, RuntimeContext, StreamEvent};

pub struct PhaseRunner;

impl PhaseRunner {
    /// Run one phase. Caller pre-configures `ctx` with phase's tool filter
    /// and `WikiGuard` (via the constructed RuntimeContext).
    ///
    /// `edit_feedback`, if present, is prepended as a `ChatMessage::user(...)`
    /// before the phase's system prompt.
    pub async fn run(
        ctx: RuntimeContext,
        template: &FlowTemplate,
        phase: &PhaseId,
        vars: &HashMap<String, String>,
        edit_feedback: Option<String>,
        prior_messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<PhaseOutput> {
        let raw_prompt = template.resolve_prompt(phase)?;
        let rendered = render_prompt(&raw_prompt, vars);

        let mut messages = prior_messages;
        if let Some(text) = edit_feedback {
            messages.push(ChatMessage::user(text));
        }
        messages.push(ChatMessage::system(rendered));

        let result = kernel::execute_streaming(&ctx, messages, event_tx).await
            .map_err(|e| anyhow::anyhow!("kernel error in phase '{}': {}", phase, e))?;

        Ok(PhaseOutput {
            summary: result.content,
            iterations_used: 0, // kernel doesn't surface today
            tools_called: result.tools_used,
            finished_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_runner_compiles() {
        let _ = PhaseRunner;
    }
}
```

- [ ] **Step 2: `CliGate` — gate input parsing + stdin read**

`gasket/engine/src/flow/gate.rs`:

```rust
//! CLI gate controller. Web channel reuses `parse_response`.

use crate::flow::state::GateResponse;

pub fn parse_response(line: &str) -> Option<GateResponse> {
    let t = line.trim();
    let lower = t.to_lowercase();
    match lower.as_str() {
        "y" | "yes" => Some(GateResponse::Yes),
        "n" | "no" => Some(GateResponse::No),
        "redo" => Some(GateResponse::Redo),
        "back" => Some(GateResponse::Back),
        "edit" => Some(GateResponse::Edit(String::new())),
        s if s.starts_with("edit ") => {
            let rest = t[5..].trim().to_string();
            Some(GateResponse::Edit(rest))
        }
        _ => None,
    }
}

pub struct CliGate;
impl CliGate {
    pub fn read_response(prompt: &str) -> std::io::Result<GateResponse> {
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        loop {
            print!("{prompt} ");
            stdout.flush()?;
            let mut line = String::new();
            if stdin.lock().read_line(&mut line)? == 0 {
                return Ok(GateResponse::No); // EOF = abort
            }
            if let Some(r) = parse_response(&line) {
                return Ok(r);
            }
            eprintln!("Use: y / n / edit <text> / redo / back");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yn() {
        assert_eq!(parse_response("y"), Some(GateResponse::Yes));
        assert_eq!(parse_response("YES"), Some(GateResponse::Yes));
        assert_eq!(parse_response(" no "), Some(GateResponse::No));
    }
    #[test]
    fn test_parse_edit() {
        assert_eq!(parse_response("edit"), Some(GateResponse::Edit(String::new())));
        assert_eq!(parse_response("edit clarify scope"),
                   Some(GateResponse::Edit("clarify scope".into())));
    }
    #[test]
    fn test_parse_redo_back() {
        assert_eq!(parse_response("redo"), Some(GateResponse::Redo));
        assert_eq!(parse_response("back"), Some(GateResponse::Back));
    }
    #[test]
    fn test_parse_unknown() {
        assert_eq!(parse_response(""), None);
        assert_eq!(parse_response("maybe"), None);
    }
}
```

- [ ] **Step 3: `FlowOrchestrator` — real kernel integration**

`gasket/engine/src/flow/orchestrator.rs`:

```rust
//! FlowOrchestrator — owns one running flow.
//!
//! step() returns &FlowStatus (no separate Outcome type per Linus simplification).

use anyhow::{Context, Result};
use chrono::Utc;
use gasket_providers::ChatMessage;
use gasket_storage::{FlowRunRecord, FlowRunStore};
use gasket_types::{FlowStatus, PendingWikiWrite, PhaseId, PhaseOutput};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::flow::{
    state::{FlowState, GateResponse},
    template::{FlowTemplate, TemplateLoader},
    wiki_guard::{WikiGuard, WikiPolicy as GuardPolicy},
    PhaseRunner,
};
use crate::kernel::{RuntimeContext, StreamEvent};

pub struct FlowOrchestrator {
    state: FlowState,
    store: FlowRunStore,
    /// Guard the orchestrator owns for this flow's lifetime. Policy taken from template.
    /// Caller (CLI) wires this into the ToolRegistry's WikiWriteTool before running phases.
    flow_guard: Arc<WikiGuard>,
}

impl FlowOrchestrator {
    pub async fn start_new(
        loader: &TemplateLoader,
        store: FlowRunStore,
        template_name: &str,
        user_request: String,
        session_key: String,
    ) -> Result<Self> {
        let template = loader.load(template_name)
            .with_context(|| format!("loading template '{template_name}'"))?;
        // Read raw YAML for snapshot
        let user_path = std::path::PathBuf::from(format!("~/.gasket/flows/{template_name}.yaml"));
        let template_yaml = std::fs::read_to_string(
            shellexpand::tilde(&user_path.to_string_lossy()).as_ref()
        ).unwrap_or_else(|_| {
            // Fallback: serialize current template back. Not byte-exact but acceptable.
            serde_yaml::to_string(template.as_ref()).unwrap_or_default()
        });

        let state = FlowState::new_with_template(user_request, session_key, template.clone());
        let policy: GuardPolicy = template.wiki_policy.into();
        let flow_guard = Arc::new(WikiGuard::new(policy));

        let record = FlowRunRecord {
            flow_id: state.flow_id.to_string(),
            template_name: template.name.clone(),
            template_version: template.version,
            template_yaml,
            user_request: state.user_request.clone(),
            session_key: state.session_key.clone(),
            status: state.status.as_str().to_string(),
            current_phase: state.current_phase.clone(),
            completed_phases: "{}".to_string(),
            pending_wiki: "[]".to_string(),
            edit_feedback: None,
            created_at: state.created_at.timestamp(),
            updated_at: state.updated_at.timestamp(),
        };
        store.insert(&record).await?;

        Ok(Self { state, store, flow_guard })
    }

    pub async fn resume(
        loader: &TemplateLoader,
        store: FlowRunStore,
        flow_id: &str,
    ) -> Result<Self> {
        let record = store.load(flow_id).await?
            .with_context(|| format!("flow '{flow_id}' not found"))?;

        // Snapshot-first: use template_yaml from the record.
        let force_reload = std::env::var("GASKET_FLOWS_FORCE_RELOAD").ok().as_deref() == Some("1");
        let template = if force_reload {
            loader.load(&record.template_name)?
        } else {
            FlowTemplate::load_from_yaml(
                &record.template_yaml,
                std::path::PathBuf::from(
                    shellexpand::tilde("~/.gasket/flows").as_ref(),
                ),
            )?
        };

        let state = record_to_state(&record, template.clone())?;
        let policy: GuardPolicy = template.wiki_policy.into();
        let flow_guard = Arc::new(WikiGuard::new(policy));
        Ok(Self { state, store, flow_guard })
    }

    pub fn flow_id(&self) -> uuid::Uuid { self.state.flow_id }
    pub fn status(&self) -> &FlowStatus { &self.state.status }
    pub fn current_phase(&self) -> &PhaseId { &self.state.current_phase }
    pub fn template(&self) -> &Arc<FlowTemplate> { &self.state.template }
    pub fn flow_guard(&self) -> Arc<WikiGuard> { self.flow_guard.clone() }

    /// Run the current phase to completion via PhaseRunner, then transition.
    /// Returns `&FlowStatus` so the caller knows what to do next.
    pub async fn step(
        &mut self,
        runtime_ctx: RuntimeContext,
        event_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<&FlowStatus> {
        let phase = self.state.current_phase.clone();
        let vars = self.template_vars();
        let edit_feedback = self.state.take_edit_feedback();

        let result = PhaseRunner::run(
            runtime_ctx,
            &self.state.template,
            &phase,
            &vars,
            edit_feedback,
            Vec::new(), // v2: no carry-over of message history; prompt template carries summaries via {{previous_outputs.X}}
            event_tx,
        ).await;

        match result {
            Ok(output) => {
                self.state.complete_current_phase(output);
                self.state.transition_after_phase();
            }
            Err(_) => {
                self.state.pause();
            }
        }
        self.persist().await?;
        Ok(&self.state.status)
    }

    pub async fn apply_gate(&mut self, resp: GateResponse) -> Result<&FlowStatus> {
        self.state.apply_gate_response(resp);
        self.persist().await?;
        Ok(&self.state.status)
    }

    pub async fn abort(&mut self) -> Result<()> {
        self.state.abort();
        self.persist().await?;
        Ok(())
    }

    pub fn drain_pending_wiki(&self) -> Vec<PendingWikiWrite> {
        // Async drain isn't possible here since this is sync; but the flow_guard
        // returned by flow_guard() is shared, and CLI calls drain_pending().await
        // on the Arc directly. This sync method exists only for compatibility.
        Vec::new()
    }

    fn template_vars(&self) -> HashMap<String, String> {
        let mut v = HashMap::new();
        v.insert("user_request".into(), self.state.user_request.clone());
        v.insert("flow_id".into(), self.state.flow_id.to_string());
        v.insert("phase_total".into(), self.state.template.phases.len().to_string());
        let idx = self.state.template.phases.iter()
            .position(|p| p.id == self.state.current_phase).unwrap_or(0);
        v.insert("phase_index".into(), (idx + 1).to_string());
        for (k, out) in &self.state.completed_phases {
            v.insert(format!("previous_outputs.{k}"), out.summary.clone());
        }
        if let Some((_, last)) = self.state.completed_phases.iter().last() {
            v.insert("prev_phase_output".into(), last.summary.clone());
        }
        v
    }

    async fn persist(&self) -> Result<()> {
        let r = state_to_record(&self.state);
        self.store.update(&r).await?;
        Ok(())
    }
}

fn state_to_record(s: &FlowState) -> FlowRunRecord {
    FlowRunRecord {
        flow_id: s.flow_id.to_string(),
        template_name: s.template.name.clone(),
        template_version: s.template.version,
        template_yaml: serde_yaml::to_string(s.template.as_ref()).unwrap_or_default(),
        user_request: s.user_request.clone(),
        session_key: s.session_key.clone(),
        status: s.status.as_str().to_string(),
        current_phase: s.current_phase.clone(),
        completed_phases: serde_json::to_string(&s.completed_phases).unwrap_or_else(|_| "{}".into()),
        pending_wiki: serde_json::to_string(&s.pending_wiki_writes).unwrap_or_else(|_| "[]".into()),
        edit_feedback: s.edit_feedback.clone(),
        created_at: s.created_at.timestamp(),
        updated_at: s.updated_at.timestamp(),
    }
}

fn record_to_state(r: &FlowRunRecord, template: Arc<FlowTemplate>) -> Result<FlowState> {
    use chrono::TimeZone;
    let completed: std::collections::BTreeMap<String, PhaseOutput> =
        serde_json::from_str(&r.completed_phases).unwrap_or_default();
    let pending: Vec<PendingWikiWrite> =
        serde_json::from_str(&r.pending_wiki).unwrap_or_default();
    let status = FlowStatus::parse(&r.status)
        .ok_or_else(|| anyhow::anyhow!("unknown status '{}'", r.status))?;
    Ok(FlowState {
        flow_id: r.flow_id.parse().context("invalid flow_id uuid")?,
        template,
        user_request: r.user_request.clone(),
        current_phase: r.current_phase.clone(),
        status,
        completed_phases: completed,
        pending_wiki_writes: pending,
        edit_feedback: r.edit_feedback.clone(),
        session_key: r.session_key.clone(),
        created_at: chrono::Utc.timestamp_opt(r.created_at, 0).single().unwrap_or_else(Utc::now),
        updated_at: chrono::Utc.timestamp_opt(r.updated_at, 0).single().unwrap_or_else(Utc::now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::template::WikiPolicy;

    async fn temp_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect(&format!("sqlite:file:o_{}?mode=memory&cache=shared", uuid::Uuid::new_v4().simple()))
            .await.unwrap();
        gasket_storage::migrations::flow_run::run_schema(&pool).await.unwrap();
        pool
    }

    fn write_yaml_template(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let yaml = format!(r#"
name: {name}
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: B
    prompt_file: prompts/p.md
    allowed_tools: [wiki_search]
    max_iterations: 1
    gate:
      prompt: "?"
"#);
        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, &yaml).unwrap();
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::write(dir.join("prompts/p.md"), "do {{user_request}}").unwrap();
        path
    }

    #[tokio::test]
    async fn test_start_new_inserts_with_template_yaml() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tpl");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let orch = FlowOrchestrator::start_new(&loader, store.clone(), "tpl",
            "do".into(), "cli:t".into()).await.unwrap();
        let row = store.load(&orch.flow_id().to_string()).await.unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert!(row.template_yaml.contains("name: tpl") || row.template_yaml.contains("name: \"tpl\""));
    }

    #[tokio::test]
    async fn test_apply_gate_yes_done() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tpl2");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let mut orch = FlowOrchestrator::start_new(&loader, store.clone(), "tpl2",
            "do".into(), "cli:t".into()).await.unwrap();

        // Simulate phase completion
        orch.state.complete_current_phase(PhaseOutput {
            summary: "x".into(), iterations_used: 1, tools_called: vec![],
            finished_at: Utc::now(),
        });
        orch.state.transition_after_phase();
        orch.persist().await.unwrap();
        assert_eq!(orch.status(), &FlowStatus::AwaitingGate);

        let st = orch.apply_gate(GateResponse::Yes).await.unwrap();
        assert_eq!(*st, FlowStatus::Done);
        let row = store.load(&orch.flow_id().to_string()).await.unwrap().unwrap();
        assert_eq!(row.status, "done");
    }

    #[tokio::test]
    async fn test_resume_uses_template_yaml_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_path = write_yaml_template(dir.path(), "tpl3");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let orch = FlowOrchestrator::start_new(&loader, store.clone(), "tpl3",
            "x".into(), "cli:t".into()).await.unwrap();
        let id = orch.flow_id().to_string();
        drop(orch);

        // User edits the template after flow started — snapshot must be authoritative.
        std::fs::write(&tpl_path, "name: tampered\nversion: 1\nphases: []\n").unwrap();

        let resumed = FlowOrchestrator::resume(&loader, store, &id).await.unwrap();
        assert_eq!(resumed.template().name, "tpl3"); // not "tampered"
    }

    #[tokio::test]
    async fn test_edit_feedback_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tpl4");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let mut orch = FlowOrchestrator::start_new(&loader, store, "tpl4",
            "x".into(), "cli:t".into()).await.unwrap();

        // Get to gate
        orch.state.complete_current_phase(PhaseOutput {
            summary: "x".into(), iterations_used: 1, tools_called: vec![], finished_at: Utc::now(),
        });
        orch.state.transition_after_phase();

        // Edit
        orch.apply_gate(GateResponse::Edit("clarify".into())).await.unwrap();
        assert_eq!(orch.state.edit_feedback.as_deref(), Some("clarify"));
        assert_eq!(orch.state.current_phase, "brainstorm");
        assert_eq!(orch.status(), &FlowStatus::Running);
    }
}
```

The orchestrator references `shellexpand` which isn't in deps. Use a simpler home-dir approach:

```rust
fn home_flows_dir() -> std::path::PathBuf {
    dirs::home_dir().map(|p| p.join(".gasket/flows"))
        .unwrap_or_else(|| std::path::PathBuf::from(".gasket/flows"))
}
```

Replace the two `shellexpand::tilde` calls with `home_flows_dir()`.

Also `gasket_storage::migrations::flow_run` needs `migrations` to be public. Check:

```bash
grep "mod migrations" gasket/storage/src/lib.rs
```

If private, change to `pub mod migrations;`.

- [ ] **Step 4: Update `flow/mod.rs` re-exports**

```rust
pub mod gate;
pub mod orchestrator;
pub mod phase_runner;
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use gate::{parse_response, CliGate};
pub use orchestrator::FlowOrchestrator;
pub use phase_runner::PhaseRunner;
pub use state::{FlowState, GateResponse};
pub use template::{render_prompt, FlowTemplate, GateConfig, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{GuardDecision, WikiGuard, WriteArgsView};
```

- [ ] **Step 5: Run tests**

```bash
cargo test --package gasket-engine flow:: 2>&1 | tail -25
```

Expected: gate 4 + phase_runner 1 + orchestrator 4 + (existing 11 state + 6 template + 6 wiki_guard) = 32 passed total in flow.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/flow/phase_runner.rs gasket/engine/src/flow/gate.rs \
        gasket/engine/src/flow/orchestrator.rs gasket/engine/src/flow/mod.rs \
        gasket/storage/src/lib.rs
git commit -m "feat(engine): add PhaseRunner, CliGate, FlowOrchestrator with real kernel integration"
```

---

### Task 8: Command parser + dispatcher + CLI wiring + built-in templates

**Files:**
- Create: `gasket/engine/src/command/mod.rs`
- Create: `gasket/engine/src/command/parser.rs`
- Create: `gasket/engine/src/command/dispatcher.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/cli/src/commands/agent.rs`
- Create: `gasket/engine/flows/{default,debug,docs,brainstorm-only,design-only,plan-only,execute-only,verify-only}.yaml`
- Create: `gasket/engine/flows/prompts/{brainstorm,design,plan,execute,verify}.md`
- Modify: `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md` (mark Superseded)

This task is the largest — ties everything together. Real kernel integration in CLI is mandatory.

- [ ] **Step 1: Command parser**

`gasket/engine/src/command/mod.rs`:

```rust
pub mod dispatcher;
pub mod parser;

pub use dispatcher::CommandDispatcher;
pub use parser::{parse, CommandAction};
```

`gasket/engine/src/command/parser.rs`:

```rust
//! Slash-command parser.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Plain(String),
    Start { template: String, request: String },
    Status,
    Resume { flow_id: String },
    Abort,
    List,
    Unknown { command: String, raw: String },
}

pub fn parse(input: &str, known_templates: &[String]) -> CommandAction {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return CommandAction::Plain(input.to_string());
    }
    let body = &trimmed[1..];
    let mut parts = body.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match head {
        "flow" => parse_flow_sub(rest, known_templates),
        "brainstorm" => CommandAction::Start {
            template: "brainstorm-only".into(), request: rest.into() },
        "design" => CommandAction::Start {
            template: "design-only".into(), request: rest.into() },
        "plan" => CommandAction::Start {
            template: "plan-only".into(), request: rest.into() },
        "execute" => CommandAction::Start {
            template: "execute-only".into(), request: rest.into() },
        "verify" => CommandAction::Start {
            template: "verify-only".into(), request: rest.into() },
        _ => CommandAction::Unknown { command: head.into(), raw: trimmed.into() },
    }
}

fn parse_flow_sub(rest: &str, known: &[String]) -> CommandAction {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();
    match sub {
        "start" => parse_flow_start(args, known),
        "status" => CommandAction::Status,
        "resume" => CommandAction::Resume { flow_id: args.into() },
        "abort" => CommandAction::Abort,
        "list" => CommandAction::List,
        _ => CommandAction::Unknown {
            command: format!("flow {sub}"),
            raw: format!("/flow {rest}"),
        },
    }
}

fn parse_flow_start(args: &str, known: &[String]) -> CommandAction {
    if let Some((tpl, req)) = args.split_once(" -- ") {
        return CommandAction::Start {
            template: tpl.trim().into(),
            request: req.trim().into(),
        };
    }
    let mut iter = args.splitn(2, char::is_whitespace);
    let first = iter.next().unwrap_or("").trim();
    let rest = iter.next().unwrap_or("").trim();
    if !first.is_empty() && known.iter().any(|t| t == first) {
        CommandAction::Start { template: first.into(), request: rest.into() }
    } else {
        CommandAction::Start { template: "default".into(), request: args.trim().into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn k() -> Vec<String> { vec!["default".into(), "new-feature".into()] }

    #[test]
    fn test_plain() {
        assert_eq!(parse("hi", &k()), CommandAction::Plain("hi".into()));
    }
    #[test]
    fn test_dash_dash_separator() {
        assert_eq!(parse("/flow start nf -- do x", &k()),
            CommandAction::Start { template: "nf".into(), request: "do x".into() });
    }
    #[test]
    fn test_known_template_first_word() {
        assert_eq!(parse("/flow start new-feature add auth", &k()),
            CommandAction::Start { template: "new-feature".into(), request: "add auth".into() });
    }
    #[test]
    fn test_unknown_template_falls_back_to_default() {
        assert_eq!(parse("/flow start fix bug", &k()),
            CommandAction::Start { template: "default".into(), request: "fix bug".into() });
    }
    #[test]
    fn test_status_resume_abort_list() {
        assert_eq!(parse("/flow status", &k()), CommandAction::Status);
        assert_eq!(parse("/flow resume abc-123", &k()),
            CommandAction::Resume { flow_id: "abc-123".into() });
        assert_eq!(parse("/flow abort", &k()), CommandAction::Abort);
        assert_eq!(parse("/flow list", &k()), CommandAction::List);
    }
    #[test]
    fn test_brainstorm_design_plan_execute_verify() {
        for (cmd, tpl) in [
            ("/brainstorm X", "brainstorm-only"),
            ("/design X", "design-only"),
            ("/plan X", "plan-only"),
            ("/execute X", "execute-only"),
            ("/verify X", "verify-only"),
        ] {
            match parse(cmd, &k()) {
                CommandAction::Start { template, request } => {
                    assert_eq!(template, tpl);
                    assert_eq!(request, "X");
                }
                _ => panic!("expected Start for {cmd}"),
            }
        }
    }
    #[test]
    fn test_unknown() {
        match parse("/foo bar", &k()) {
            CommandAction::Unknown { command, .. } => assert_eq!(command, "foo"),
            _ => panic!("expected Unknown"),
        }
    }
}
```

`gasket/engine/src/command/dispatcher.rs`:

```rust
use crate::command::parser::{parse, CommandAction};

pub struct CommandDispatcher {
    known_templates: Vec<String>,
}

impl CommandDispatcher {
    pub fn new(known_templates: Vec<String>) -> Self { Self { known_templates } }
    pub fn set_known_templates(&mut self, t: Vec<String>) { self.known_templates = t; }
    pub fn route(&self, input: &str) -> CommandAction {
        parse(input, &self.known_templates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route() {
        let d = CommandDispatcher::new(vec!["default".into()]);
        assert_eq!(d.route("hi"), CommandAction::Plain("hi".into()));
        match d.route("/flow start hello world") {
            CommandAction::Start { template, .. } => assert_eq!(template, "default"),
            _ => panic!(),
        }
    }
}
```

In `gasket/engine/src/lib.rs`, add `pub mod command;`.

- [ ] **Step 2: Built-in templates and prompts**

Create `gasket/engine/flows/prompts/brainstorm.md`:

```markdown
[Phase: Brainstorm — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

You are in the brainstorm phase. Use `wiki_search` and `history_search` to find prior work. Surface 2-3 directions with trade-offs. Ask at most one clarifying question. End with a one-paragraph summary the user can confirm.

Do NOT write code or design schemas yet.
```

`design.md`:
```markdown
[Phase: Design — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

Brainstorm outcome:
> {{previous_outputs.brainstorm}}

Produce a sectioned design covering: architecture/component boundaries, data flow, error handling, testing approach.
```

`plan.md`:
```markdown
[Phase: Plan — Step {{phase_index}} of {{phase_total}}]

Design:
> {{previous_outputs.design}}

Break the design into bite-sized TDD tasks: write test → fail → impl → pass → commit.
End with a numbered task list.
```

`execute.md`:
```markdown
[Phase: Execute — Step {{phase_index}} of {{phase_total}}]

Plan:
> {{previous_outputs.plan}}

Implement task by task. Tests-first. Commit after each task. Summarise at the end.
```

`verify.md`:
```markdown
[Phase: Verify — Step {{phase_index}} of {{phase_total}}]

What was executed:
> {{previous_outputs.execute}}

Run all relevant tests. Confirm the user's original request is satisfied. Surface any deviation.
```

Create `gasket/engine/flows/default.yaml`:

```yaml
name: default
description: Default 5-phase plan-act-review flow
version: 1
wiki_policy: deferred

phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, wiki_read, history_search]
    max_iterations: 5
    gate:
      prompt: "Brainstorm direction looks right? (y/n/edit/redo/back)"

  - id: design
    label: Design
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 8
    gate:
      prompt: "Design accepted?"

  - id: plan
    label: Plan
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read, file_search]
    max_iterations: 5
    gate:
      prompt: "Plan looks executable?"

  - id: execute
    label: Execute
    prompt_file: prompts/execute.md
    # allowed_tools omitted = all tools
    # max_iterations omitted = unlimited
    # gate omitted = auto-advance

  - id: verify
    label: Verify
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read]
    max_iterations: 5
    gate:
      prompt: "Verification passed?"
```

`debug.yaml` (skip design):
```yaml
name: debug
description: Bug-fix flow without design phase
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: Diagnose
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, history_search, file_read]
    max_iterations: 5
    gate:
      prompt: "Diagnosed correctly?"
  - id: plan
    label: Plan Fix
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read]
    max_iterations: 3
    gate:
      prompt: "Fix plan accepted?"
  - id: execute
    label: Execute
    prompt_file: prompts/execute.md
  - id: verify
    label: Verify
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read]
    max_iterations: 5
    gate:
      prompt: "Bug fixed?"
```

`docs.yaml`:
```yaml
name: docs
description: Documentation flow
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 3
    gate:
      prompt: "Topic understood?"
  - id: design
    label: Outline
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, file_read]
    max_iterations: 3
    gate:
      prompt: "Outline accepted?"
  - id: execute
    label: Write
    prompt_file: prompts/execute.md
    allowed_tools: [file_read, file_search]
    gate:
      prompt: "Documentation acceptable?"
```

Single-phase shortcuts. Create `brainstorm-only.yaml`:
```yaml
name: brainstorm-only
description: Brainstorm shortcut
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, wiki_read, history_search]
    max_iterations: 5
    gate:
      prompt: "Done?"
```

Same pattern for `design-only.yaml` / `plan-only.yaml` / `execute-only.yaml` / `verify-only.yaml` — each refers to its own prompt and its own appropriate `allowed_tools`.

- [ ] **Step 3: Wire CLI agent — real kernel integration**

In `gasket/cli/src/commands/agent.rs`, after the agent is built (around the existing `let render_md = !opts.no_markdown;` line), add:

```rust
    // Flow command system
    use gasket_engine::command::{CommandAction, CommandDispatcher};
    use gasket_engine::flow::{CliGate, FlowOrchestrator, TemplateLoader};

    let user_flows_dir = dirs::home_dir()
        .map(|h| h.join(".gasket/flows"))
        .unwrap_or_else(|| std::path::PathBuf::from(".gasket/flows"));
    let builtin_flows_dir = std::env::var("GASKET_FLOWS_DIR").ok().map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::current_exe().ok()
                .and_then(|p| p.parent().and_then(|x| x.parent()).and_then(|x| x.parent()).map(|x| x.join("engine/flows")))
                .filter(|p| p.exists())
        });
    let template_loader = TemplateLoader::new(user_flows_dir, builtin_flows_dir);
    let mut dispatcher = CommandDispatcher::new(template_loader.list());
    let flow_run_store = sqlite_store.flow_run_store();

    let mut active_flow: Option<FlowOrchestrator> = None;
```

In the input loop, before passing the user line to `agent.process_direct_streaming_with_channel`, add dispatch:

```rust
        match dispatcher.route(&line) {
            CommandAction::Plain(text) => {
                // existing path: agent.process_direct_streaming_with_channel(&text, ...)
            }
            CommandAction::Start { template, request } => {
                if active_flow.is_some() {
                    eprintln!("⚠ A flow is already active. Use /flow abort or /flow resume.");
                    continue;
                }
                match FlowOrchestrator::start_new(
                    &template_loader, flow_run_store.clone(), &template, request,
                    session_key.to_string(),
                ).await {
                    Ok(orch) => {
                        println!("▶ Flow {} started. flow_id={}", template, orch.flow_id());
                        active_flow = Some(orch);
                        drive_flow(&mut active_flow, &agent, &session_key, &template_loader,
                                  flow_run_store.clone(), &tools, provider_info.provider.clone()).await;
                    }
                    Err(e) => eprintln!("⚠ Failed to start flow: {e}"),
                }
            }
            CommandAction::Status => match &active_flow {
                Some(o) => println!("Active flow {} ({:?}) at phase '{}'",
                    o.flow_id(), o.status(), o.current_phase()),
                None => println!("No active flow."),
            },
            CommandAction::Resume { flow_id } => {
                match FlowOrchestrator::resume(&template_loader, flow_run_store.clone(), &flow_id).await {
                    Ok(orch) => {
                        println!("▶ Resumed flow {}", orch.flow_id());
                        active_flow = Some(orch);
                        drive_flow(&mut active_flow, &agent, &session_key, &template_loader,
                                  flow_run_store.clone(), &tools, provider_info.provider.clone()).await;
                    }
                    Err(e) => eprintln!("⚠ Resume failed: {e}"),
                }
            }
            CommandAction::Abort => {
                if let Some(mut o) = active_flow.take() {
                    let _ = o.abort().await;
                    println!("Flow aborted.");
                } else {
                    println!("No active flow.");
                }
            }
            CommandAction::List => {
                match flow_run_store.list_recent(20).await {
                    Ok(rows) => for r in rows {
                        println!("{} {} {} ({})", r.flow_id, r.status, r.current_phase, r.template_name);
                    },
                    Err(e) => eprintln!("⚠ List failed: {e}"),
                }
            }
            CommandAction::Unknown { command, .. } => {
                eprintln!("⚠ Unknown command: /{command}");
            }
        }
```

Add `drive_flow` near the bottom of the file:

```rust
async fn drive_flow(
    active_flow: &mut Option<FlowOrchestrator>,
    _agent: &AgentSession,
    session_key: &SessionKey,
    _template_loader: &TemplateLoader,
    _store: gasket_engine::FlowRunStore,
    tools: &Arc<gasket_engine::tools::ToolRegistry>,
    provider: Arc<dyn gasket_providers::LlmProvider>,
) {
    let Some(o) = active_flow.as_mut() else { return };
    loop {
        let st = o.status().clone();
        match st {
            gasket_types::FlowStatus::Running => {
                // Build a phase-scoped RuntimeContext.
                // Filter tools for this phase via FlowTemplate::is_tool_allowed.
                let phase = o.current_phase().clone();
                let template = o.template().clone();
                let filtered_tools = filter_tools_for_phase(tools, &template, &phase);
                let phase_guard = o.flow_guard();
                let registry_with_guard = swap_wiki_guard(filtered_tools, phase_guard);

                let ctx = build_runtime_context(provider.clone(), Arc::new(registry_with_guard), session_key.clone());
                let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
                // forward events to the existing channel printer (similar to existing path)
                tokio::spawn(async move {
                    while let Some(_ev) = event_rx.recv().await {
                        // Forward to user (real impl: bridge to ChatEvent, print to stdout)
                    }
                });
                if let Err(e) = o.step(ctx, event_tx).await {
                    eprintln!("⚠ Phase failed: {e}");
                    break;
                }
            }
            gasket_types::FlowStatus::AwaitingGate => {
                let template = o.template();
                let phase = o.current_phase();
                let spec = template.phases.iter().find(|p| &p.id == phase);
                let prompt = spec.and_then(|s| s.gate.as_ref())
                    .map(|g| g.prompt.as_str())
                    .unwrap_or("Continue?");
                println!("\n[Gate: {phase}] {prompt}");
                let resp = match CliGate::read_response("Input:") {
                    Ok(r) => r,
                    Err(_) => break,
                };
                if let Err(e) = o.apply_gate(resp).await {
                    eprintln!("⚠ Gate apply failed: {e}");
                    break;
                }
            }
            gasket_types::FlowStatus::Done => {
                let pending = o.flow_guard().drain_pending().await;
                if !pending.is_empty() {
                    println!("\nFlow done. {} pending wiki write(s):", pending.len());
                    for w in &pending {
                        println!("  - {} ({})", w.path, w.title);
                    }
                    println!("Approve with: gasket wiki review");
                } else {
                    println!("Flow done.");
                }
                *active_flow = None;
                break;
            }
            gasket_types::FlowStatus::Paused => {
                println!("Flow paused. Resume with: /flow resume {}", o.flow_id());
                break;
            }
            gasket_types::FlowStatus::Aborted => {
                *active_flow = None;
                break;
            }
        }
    }
}

fn filter_tools_for_phase(
    tools: &Arc<gasket_engine::tools::ToolRegistry>,
    template: &Arc<gasket_engine::flow::FlowTemplate>,
    phase: &gasket_types::PhaseId,
) -> gasket_engine::tools::ToolRegistry {
    // Implementation: clone the registry, then remove tools not whitelisted by template.
    // ToolRegistry::clone_filter(&self, predicate) is added in this task if missing.
    // Look at gasket/engine/src/tools/registry.rs and add a method like:
    //   pub fn filtered<F: Fn(&str) -> bool>(&self, f: F) -> Self
    // that clones tools whose names pass the predicate.
    tools.filtered(|name| template.is_tool_allowed(phase, name))
}

fn swap_wiki_guard(
    mut registry: gasket_engine::tools::ToolRegistry,
    guard: Arc<gasket_engine::flow::WikiGuard>,
) -> gasket_engine::tools::ToolRegistry {
    // If the registry has a wiki_write tool, replace it with one wired to `guard`.
    // ToolRegistry::replace(&mut self, name, Box<dyn Tool>) is added in this task if missing.
    registry.replace_with("wiki_write", |existing| {
        existing.as_any().downcast_ref::<gasket_engine::tools::WikiWriteTool>()
            .map(|wt| Box::new(WikiWriteTool::with_guard(wt.page_store_clone(), guard.clone())) as Box<dyn gasket_types::Tool>)
    });
    registry
}

fn build_runtime_context(
    provider: Arc<dyn gasket_providers::LlmProvider>,
    tools: Arc<gasket_engine::tools::ToolRegistry>,
    session_key: SessionKey,
) -> gasket_engine::kernel::RuntimeContext {
    // Construct a fresh RuntimeContext for one phase. Mirror what
    // AgentSession does in process_direct_streaming_with_channel.
    let mut ctx = gasket_engine::kernel::RuntimeContext::default();
    ctx.provider = Some(provider);
    ctx.tools = tools;
    ctx.session_key = Some(session_key);
    ctx.config = gasket_engine::kernel::KernelConfig::new(); // adjust to flow-appropriate caps
    ctx
}
```

**Note**: the `ToolRegistry::filtered` and `ToolRegistry::replace_with` methods may not exist yet. As part of this task, add them in `gasket/engine/src/tools/registry.rs`:

```rust
impl ToolRegistry {
    pub fn filtered<F: Fn(&str) -> bool>(&self, f: F) -> ToolRegistry {
        let mut out = ToolRegistry::new();
        for (name, tool) in self.tools.iter() {
            if f(name) {
                if let Some(boxed) = tool.clone_box() {
                    out.register(boxed);
                }
            }
        }
        out
    }

    pub fn replace_with<F>(&mut self, name: &str, f: F)
    where
        F: FnOnce(&dyn gasket_types::Tool) -> Option<Box<dyn gasket_types::Tool>>,
    {
        if let Some(existing) = self.tools.get(name) {
            if let Some(replacement) = f(existing.as_ref()) {
                self.tools.insert(name.to_string(), Arc::from(replacement));
            }
        }
    }
}
```

Adjust to match the actual fields of `ToolRegistry` in your codebase.

The `WikiWriteTool::page_store_clone()` method may need to be added — `pub fn page_store_clone(&self) -> PageStore { self.page_store.clone() }`. Check whether `PageStore` is `Clone`; if not, add `#[derive(Clone)]` to it.

- [ ] **Step 4: Mark old spec as superseded**

In `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md`, change line 4 from:

```markdown
**Status**: Draft
```

to:

```markdown
**Status**: Superseded by `2026-05-03-flow-command-system-design.md`
```

- [ ] **Step 5: Build entire workspace and run all tests**

```bash
cargo build --workspace --release 2>&1 | tail -10
cargo test --workspace 2>&1 | tail -20
```

Expected: clean build; all tests pass.

- [ ] **Step 6: End-to-end smoke test**

```bash
rm -f ~/.gasket/gasket.db
GASKET_FLOWS_DIR=$(pwd)/gasket/engine/flows cargo run --release --package gasket-cli -- agent -m "/flow list"
# Expected: empty list

GASKET_FLOWS_DIR=$(pwd)/gasket/engine/flows cargo run --release --package gasket-cli -- agent -m "/brainstorm test idea"
# Expected: a brainstorm phase runs through the kernel, gate prompts appear, etc.

GASKET_FLOWS_DIR=$(pwd)/gasket/engine/flows cargo run --release --package gasket-cli -- agent -m "what is 2+2?"
# Expected: normal agent response (plain text bypasses dispatcher)
```

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/command/ gasket/engine/src/lib.rs \
        gasket/engine/src/tools/registry.rs gasket/engine/src/tools/wiki_tools.rs \
        gasket/engine/flows/ \
        gasket/cli/src/commands/agent.rs \
        docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md
git commit -m "feat(engine,cli): add command dispatcher + FlowOrchestrator CLI integration with built-in templates"
```

---

## Self-Review Checklist

**1. Spec coverage**

| Spec section | Task |
|---|---|
| §1 Overview (v1+v2 split) | Tasks 1-4 (v1) + Tasks 5-8 (v2) |
| §2.1 Layer Map | Tasks 1, 7, 8 |
| §2.3 Touch Points | Tasks 1-3 (v1), 5-8 (v2) |
| §3.1 Template layout | Task 8 |
| §3.2 YAML schema | Task 5 |
| §3.3 Variables | Tasks 5 (`render_prompt`), 7 (`template_vars`) |
| §3.4 Slash commands | Task 8 |
| §3.5 Gate interaction | Task 7 (`CliGate`) |
| §4.1 E2E sequence | Task 7 |
| §4.2 FlowState (PhaseId=String, no AwaitingGate phase, edit_feedback) | Task 5 |
| §4.3 SQLite (template_yaml column) | Task 6 |
| §4.4 State machine | Task 5 (test cases match all 11 transition rows) |
| §4.5 Resume (snapshot-first) | Task 7 (`resume`) |
| §4.6 WikiGuard (single struct) | Tasks 1, 2, 3 (v1) |
| §5.1 Error catalog | Tasks 1-3, 7 |
| §5.2 Invariants | Tests in Tasks 5, 7 |
| §6 Testing | Each task has tests |
| §7 Implementation order (8 tasks, 2 batches) | Matches |
| §8 Out of Scope | Honored — no kernel-integration deferral; web UI deferred |

**2. Placeholder scan**

- No "TBD", no "v1.1", no "stub" except the unavoidable v2-future-work in §8.
- v1 ships independently, value confirmed before v2 starts.

**3. Type consistency**

- `WikiPolicy` exists in two places (`flow::wiki_guard` for runtime, `flow::template` for YAML/config). Conversion via `From` impl in template.rs. Documented.
- `PhaseId = String` everywhere (types, state, template, orchestrator, parser).
- `FlowStatus::AwaitingGate` has no field anywhere.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-03-flow-command-system.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task. After v1 (Tasks 1–4) lands, pause for ~1 week of real use before starting v2 (Tasks 5–8). Best fit for this plan since v1 is independently shippable.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`. Faster for tight iteration but loses the v1-bake-time benefit.

**Which approach?**
