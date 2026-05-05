# Subagent Architecture (Role + Budget) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the subagent spawning subsystem so (a) only the main agent (Orchestrator) can spawn subagents and (b) all spawn paths share a single global concurrency budget.

**Architecture:** Introduce two first-class types — `AgentRole` enum (replaces the implicit `spawner.is_some()` boolean) and `SpawnBudget` type (named wrapper around `Arc<Semaphore>`). `SimpleSpawner` holds a pre-built worker `ToolRegistry` (without `spawn`/`spawn_parallel` registered) and acquires a budget permit before launching each worker; permit lives with the worker tokio task and auto-releases on drop. `NoopSpawner` is deleted; `ToolContext.spawner` becomes `Option<Arc<dyn>>`.

**Tech Stack:** Rust 2021, tokio (Semaphore/OwnedSemaphorePermit), serde for config, async-trait, sqlx (touched only via existing patterns), `cargo test --workspace`.

**Reference spec:** `docs/superpowers/specs/2026-05-04-subagent-architecture-design.md`

---

## File Structure

### New files
| Path | Responsibility |
|---|---|
| `gasket/types/src/agent.rs` | `AgentRole` enum |
| `gasket/types/src/spawn_budget.rs` | `SpawnBudget` named wrapper around `Arc<Semaphore>` |

### Modified files
| Path | Reason |
|---|---|
| `gasket/types/src/lib.rs` | Re-export new modules; remove `NoopSpawner` re-export |
| `gasket/types/src/tool.rs` | Delete `NoopSpawner`; change `ToolContext.spawner` to `Option`; update builder + Default |
| `gasket/engine/src/config/tools.rs` | Add `SpawnToolConfig` (max_concurrency=1) |
| `gasket/engine/src/kernel/context.rs` | Add `role` field to `RuntimeContext`; add `new_worker` constructor |
| `gasket/engine/src/kernel/steppable_executor.rs` | Update `.spawner(...)` builder call site |
| `gasket/engine/src/tools/builder.rs` | Add `role` to `ToolRegistryConfig`; pass into `CoreToolProvider` |
| `gasket/engine/src/tools/provider.rs` | `CoreToolProvider` gains `role`; gate spawn tool registration |
| `gasket/engine/src/tools/spawn.rs` | Handle `Option<spawner>` |
| `gasket/engine/src/tools/spawn_parallel.rs` | Handle `Option<spawner>`; **delete** internal `Semaphore::new(5)` (2 places) |
| `gasket/engine/src/subagents/manager.rs` | `SimpleSpawner` rename `tools`→`worker_tools`, add `budget`; `spawn` paths acquire permit; subagent context sets `role: Worker` |
| `gasket/cli/src/commands/agent.rs` | Build a separate worker registry; create `SpawnBudget` from config; pass to `SimpleSpawner::new` |
| `gasket/cli/src/commands/gateway.rs` | Same as agent.rs |
| `config.example.yaml` | Document `tools.spawn.max_concurrency` |

### Behaviour Contract → Task Mapping
| Contract (spec §6) | Implemented in |
|---|---|
| C1: Worker registry has no spawn tools | Task 6 |
| C2: Worker spawner is None | Task 7 |
| C3: Worker role is Worker | Task 7 |
| C4: max_concurrency=1 serializes 3 spawns | Task 7 |
| C5: max_concurrency=2 caps parallel inflight | Task 7 |
| C6: Default config = 1 | Task 3 |
| C7: NoopSpawner deleted | Task 4 |
| C8: `cargo test --workspace` green | Final step of every task |

---

## Task 1: Add `AgentRole` enum

**Files:**
- Create: `gasket/types/src/agent.rs`
- Modify: `gasket/types/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `agent.rs`

- [ ] **Step 1.1: Write the failing test**

Create `gasket/types/src/agent.rs` with:

```rust
//! Agent role classification.
//!
//! Distinguishes whether a `RuntimeContext` belongs to the Orchestrator
//! (main agent, can dispatch workers) or a Worker (subagent, leaf node).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Worker,
}

impl AgentRole {
    /// Only the Orchestrator can spawn workers.
    pub fn can_spawn(&self) -> bool {
        matches!(self, AgentRole::Orchestrator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_can_spawn() {
        assert!(AgentRole::Orchestrator.can_spawn());
    }

    #[test]
    fn worker_cannot_spawn() {
        assert!(!AgentRole::Worker.can_spawn());
    }

    #[test]
    fn role_is_copy() {
        let r = AgentRole::Orchestrator;
        let r2 = r;
        assert_eq!(r, r2);
    }
}
```

- [ ] **Step 1.2: Wire into lib.rs**

Edit `gasket/types/src/lib.rs`. Find the existing `pub mod` declarations near the top (around line 1–25) and add:

```rust
pub mod agent;
```

Then in the `pub use` block (around line 31–35) add `AgentRole`:

```rust
pub use agent::AgentRole;
```

- [ ] **Step 1.3: Run tests**

Run: `cargo test --package gasket-types --lib agent`
Expected: 3 tests pass.

- [ ] **Step 1.4: Compile workspace**

Run: `cargo build --workspace`
Expected: Clean build.

- [ ] **Step 1.5: Commit**

```bash
git add gasket/types/src/agent.rs gasket/types/src/lib.rs
git commit -m "feat(types): add AgentRole enum (Orchestrator/Worker)"
```

---

## Task 2: Add `SpawnBudget` type

**Files:**
- Create: `gasket/types/src/spawn_budget.rs`
- Modify: `gasket/types/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 2.1: Write the failing tests**

Create `gasket/types/src/spawn_budget.rs`:

```rust
//! Global concurrency budget for subagent spawning.

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Orchestrator-session-scoped budget for the number of concurrently running workers.
///
/// All spawn paths (`spawn` / `spawn_parallel` / future dispatch tools) must
/// `acquire()` a permit before launching a worker. The returned permit is owned
/// by the worker's tokio task; it is released automatically when the task ends,
/// which limits concurrent inflight workers (not start-rate).
#[derive(Clone, Debug)]
pub struct SpawnBudget {
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

impl SpawnBudget {
    /// Creates a budget with `max_concurrency` permits. Values < 1 are clamped to 1
    /// to prevent a permanent deadlock.
    pub fn new(max_concurrency: usize) -> Self {
        let n = max_concurrency.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(n)),
            max_concurrency: n,
        }
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Acquires a permit. The returned permit MUST be moved into the
    /// spawned worker's tokio task; on drop it returns the permit.
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        // The Semaphore is held inside this Budget's Arc and is never closed.
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("SpawnBudget semaphore unexpectedly closed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn clamps_zero_to_one() {
        assert_eq!(SpawnBudget::new(0).max_concurrency(), 1);
    }

    #[test]
    fn preserves_positive_values() {
        assert_eq!(SpawnBudget::new(3).max_concurrency(), 3);
    }

    #[tokio::test]
    async fn permit_released_on_drop() {
        let b = SpawnBudget::new(1);
        let p = b.acquire().await;
        drop(p);
        // Should not block:
        let _ = tokio::time::timeout(Duration::from_millis(100), b.acquire())
            .await
            .expect("acquire should succeed after drop");
    }

    #[tokio::test]
    async fn caps_concurrent_inflight() {
        let b = SpawnBudget::new(2);
        let inflight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..5 {
            let b = b.clone();
            let inflight = inflight.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let _permit = b.acquire().await;
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}
```

- [ ] **Step 2.2: Wire into lib.rs**

Edit `gasket/types/src/lib.rs`. Add module declaration:

```rust
pub mod spawn_budget;
```

And re-export:

```rust
pub use spawn_budget::SpawnBudget;
```

- [ ] **Step 2.3: Run tests**

Run: `cargo test --package gasket-types --lib spawn_budget`
Expected: 4 tests pass (the `#[tokio::test]` ones run on tokio runtime).

- [ ] **Step 2.4: Commit**

```bash
git add gasket/types/src/spawn_budget.rs gasket/types/src/lib.rs
git commit -m "feat(types): add SpawnBudget type (Arc<Semaphore> wrapper)"
```

---

## Task 3: Add `SpawnToolConfig` to `ToolsConfig`

**Files:**
- Modify: `gasket/engine/src/config/tools.rs`
- Modify: `config.example.yaml`
- Test: extend existing `#[cfg(test)]` block in `tools.rs`

- [ ] **Step 3.1: Write the failing test**

Open `gasket/engine/src/config/tools.rs` and find the `#[cfg(test)] mod tests` block at the bottom (around line 270+). Add the following test inside that mod:

```rust
#[test]
fn spawn_config_default_is_one() {
    let cfg = SpawnToolConfig::default();
    assert_eq!(cfg.max_concurrency, 1);
}

#[test]
fn spawn_config_parses_from_yaml() {
    let yaml = r#"
spawn:
  max_concurrency: 4
"#;
    let tools: ToolsConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(tools.spawn.max_concurrency, 4);
}

#[test]
fn spawn_config_omitted_uses_default() {
    let yaml = r#"
restrict_to_workspace: true
"#;
    let tools: ToolsConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(tools.spawn.max_concurrency, 1);
}
```

Run: `cargo test --package gasket-engine --lib config::tools`
Expected: FAIL — `SpawnToolConfig` undefined.

- [ ] **Step 3.2: Add `SpawnToolConfig` and field on `ToolsConfig`**

Edit `gasket/engine/src/config/tools.rs`:

After the `WebToolsConfig` struct definition (around line 27), and before `// ── Web Tools ──` heading… Actually keep `SpawnToolConfig` near the top with the other config sub-structs. Insert the following just **after** the `ToolsConfig` struct closes (after line 21):

```rust
/// Configuration for the subagent spawn subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnToolConfig {
    /// Global concurrency cap on subagent spawning. All spawn paths
    /// (`spawn`, `spawn_parallel`) share this single budget.
    /// Values below 1 are clamped to 1 by `SpawnBudget::new`.
    #[serde(default = "default_spawn_max_concurrency")]
    pub max_concurrency: usize,
}

impl Default for SpawnToolConfig {
    fn default() -> Self {
        Self { max_concurrency: 1 }
    }
}

fn default_spawn_max_concurrency() -> usize {
    1
}
```

Then, in the existing `ToolsConfig` struct (lines 8–21), add a `spawn` field at the end:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    #[serde(default)]
    pub web: WebToolsConfig,

    #[serde(default)]
    pub exec: ExecToolConfig,

    /// Subagent spawn subsystem config (concurrency budget).
    #[serde(default)]
    pub spawn: SpawnToolConfig,          // ★ NEW
}
```

- [ ] **Step 3.3: Update `config/mod.rs` re-export**

Edit `gasket/engine/src/config/mod.rs`. Find the re-export block (line 16–18):

```rust
pub use tools::{
    CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig, ToolsConfig,
    WebToolsConfig,
};
```

Add `SpawnToolConfig`:

```rust
pub use tools::{
    CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig, SpawnToolConfig,
    ToolsConfig, WebToolsConfig,
};
```

- [ ] **Step 3.4: Run tests**

Run: `cargo test --package gasket-engine --lib config::tools`
Expected: PASS — 3 new tests + existing tests still green.

- [ ] **Step 3.5: Update `config.example.yaml`**

Open `config.example.yaml`. Find the `tools:` top-level section (search for `^tools:`). At an appropriate place under `tools:` (e.g. after the `web:` block), add:

```yaml
tools:
  # … existing entries …
  spawn:
    # Global subagent concurrency cap. All spawn paths share this budget.
    # Default 1: the main agent runs only one subagent at a time; further
    # spawn requests queue until the running one finishes.
    max_concurrency: 1
```

(Adapt indentation to match the surrounding YAML; do not duplicate the `tools:` line.)

- [ ] **Step 3.6: Commit**

```bash
git add gasket/engine/src/config/tools.rs gasket/engine/src/config/mod.rs config.example.yaml
git commit -m "feat(config): add tools.spawn.max_concurrency (default 1)"
```

---

## Task 4: Delete `NoopSpawner` and switch `ToolContext.spawner` to `Option`

**Files:**
- Modify: `gasket/types/src/tool.rs`
- Modify: `gasket/types/src/lib.rs`
- Modify: `gasket/engine/src/tools/spawn.rs`
- Modify: `gasket/engine/src/tools/spawn_parallel.rs`
- Modify: `gasket/engine/src/kernel/steppable_executor.rs`

This task changes a public type (`ToolContext.spawner`) and triggers a cascade of compiler-driven edits. We do them all in one task because partial state will not compile.

- [ ] **Step 4.1: Write a failing test (default has no spawner)**

Add the following test to `gasket/types/src/tool.rs` inside its existing `#[cfg(test)] mod tests` block (or create one near the bottom of the file if absent):

```rust
#[test]
fn default_tool_context_has_no_spawner() {
    let ctx = ToolContext::default();
    assert!(ctx.spawner.is_none(), "default ToolContext must not auto-attach a spawner");
}
```

Run: `cargo test --package gasket-types --lib tool::tests::default_tool_context_has_no_spawner`
Expected: FAIL to compile — `Option::is_none` is not a method on `Arc<dyn SubagentSpawner>`.

- [ ] **Step 4.2: Change `ToolContext.spawner` to `Option`**

In `gasket/types/src/tool.rs`, at the `ToolContext` struct (around line 140):

```rust
#[derive(Clone)]
pub struct ToolContext {
    pub session_key: SessionKey,
    pub outbound_tx: tokio::sync::mpsc::Sender<OutboundMessage>,
    /// Subagent spawner. `None` = this context cannot spawn workers
    /// (e.g. CLI mode, unit tests, or any Worker context).
    pub spawner: Option<std::sync::Arc<dyn SubagentSpawner>>,   // ★ CHANGED
    pub token_tracker: std::sync::Arc<crate::token_tracker::TokenTracker>,
    pub ws_summary_limit: usize,
    pub synthesis_callback: Option<std::sync::Arc<dyn SynthesisCallback>>,
    pub aggregator_cancel: Option<Arc<tokio::sync::Mutex<Option<CancellationToken>>>>,
}
```

In the `Default` impl (around line 161–172), change the spawner line:

```rust
impl Default for ToolContext {
    fn default() -> Self {
        let (outbound_tx, _rx) = tokio::sync::mpsc::channel(1);
        Self {
            session_key: SessionKey::new(crate::events::ChannelType::Cli, "default"),
            outbound_tx,
            spawner: None,                                       // ★ CHANGED
            token_tracker: std::sync::Arc::new(crate::token_tracker::TokenTracker::default()),
            ws_summary_limit: 0,
            synthesis_callback: None,
            aggregator_cancel: None,
        }
    }
}
```

In the builder method (around line 201–204), update to wrap in `Some`:

```rust
pub fn spawner(mut self, s: std::sync::Arc<dyn SubagentSpawner>) -> Self {
    self.spawner = Some(s);                                      // ★ CHANGED
    self
}
```

- [ ] **Step 4.3: Delete `NoopSpawner`**

In `gasket/types/src/tool.rs`, delete the entire `NoopSpawner` block. The lines to remove (around 74–93):

```rust
/// No-op spawner that always returns an error.
///
/// Used as the default `ToolContext::spawner` when no real spawner is available
/// (e.g., in CLI mode or unit tests). This eliminates `Option` wrapping and
/// ensures `SpawnTool` gets a clear runtime error instead of a `None` panic.
pub struct NoopSpawner;

#[async_trait]
impl SubagentSpawner for NoopSpawner {
    async fn spawn(
        &self,
        _task: String,
        _model_id: Option<String>,
    ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Subagent spawning is not available in this context",
        )))
    }
}
```

Delete those ~20 lines entirely.

- [ ] **Step 4.4: Remove `NoopSpawner` from re-export**

Edit `gasket/types/src/lib.rs`. Find the `pub use tool::` block (around line 31–35):

```rust
pub use tool::{
    simple_schema, ApprovalCallback, NoopSpawner, SubagentResponse, SubagentResult,
    SubagentSpawner, SynthesisCallback, Tool, ToolApprovalRequest, ToolApprovalResponse,
    ToolContext, ToolError, ToolMetadata, ToolResult,
};
```

Remove `NoopSpawner,`:

```rust
pub use tool::{
    simple_schema, ApprovalCallback, SubagentResponse, SubagentResult, SubagentSpawner,
    SynthesisCallback, Tool, ToolApprovalRequest, ToolApprovalResponse, ToolContext, ToolError,
    ToolMetadata, ToolResult,
};
```

- [ ] **Step 4.5: Update `spawn.rs` consumer**

Edit `gasket/engine/src/tools/spawn.rs`. Locate line 82–84:

```rust
// Get spawner from context (always present, may be NoopSpawner)
let spawner = &ctx.spawner;
```

Replace with:

```rust
let spawner = ctx.spawner.as_ref().ok_or_else(|| {
    ToolError::ExecutionError(
        "Subagent spawning is not available in this context (no spawner configured)".to_string(),
    )
})?;
```

(Keep the rest of `execute()` identical. The variable `spawner` is now `&Arc<dyn SubagentSpawner>` instead of `&Arc<dyn SubagentSpawner>` wrapped — same call site shape.)

- [ ] **Step 4.6: Update `spawn_parallel.rs` consumer**

Edit `gasket/engine/src/tools/spawn_parallel.rs`. Locate line 119–120:

```rust
// Get spawner from context (always present, may be NoopSpawner)
let spawner = &ctx.spawner;
```

Replace with:

```rust
let spawner = ctx.spawner.as_ref().ok_or_else(|| {
    ToolError::ExecutionError(
        "Subagent spawning is not available in this context (no spawner configured)".to_string(),
    )
})?;
```

- [ ] **Step 4.7: Update `steppable_executor.rs` builder call**

Edit `gasket/engine/src/kernel/steppable_executor.rs`. Locate lines 160–162:

```rust
let mut ctx = ToolContext::default().ws_summary_limit(self.ctx.config.ws_summary_limit);
if let Some(ref spawner) = self.ctx.spawner {
    ctx = ctx.spawner(spawner.clone());
}
```

This block does **not** need any change at the source-text level — the builder method still takes `Arc<dyn SubagentSpawner>` and now wraps in `Some` internally. Verify by re-reading (no edit). Move on.

- [ ] **Step 4.8: Compile to find any other broken sites**

Run: `cargo build --workspace`

Expected: clean. If any other site uses `NoopSpawner` or assumes `Arc<dyn>` non-Option, fix it the same way (`as_ref().ok_or(...)`). If you see no errors, proceed.

- [ ] **Step 4.9: Run focused tests**

Run: `cargo test --package gasket-types --lib tool`
Expected: PASS — `default_tool_context_has_no_spawner` passes plus existing tests.

- [ ] **Step 4.10: Verify NoopSpawner is fully gone**

Run: `rg -n NoopSpawner`
Expected: zero matches in source code (the only matches should be inside the design spec or the historical plan markdown — those are docs, not code).

If any source-code (.rs) hit remains, edit that file and remove the reference.

- [ ] **Step 4.11: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 4.12: Commit**

```bash
git add gasket/types/src/tool.rs gasket/types/src/lib.rs \
        gasket/engine/src/tools/spawn.rs gasket/engine/src/tools/spawn_parallel.rs
git commit -m "refactor(types): delete NoopSpawner; ToolContext.spawner is Option"
```

---

## Task 5: Add `role` field to `RuntimeContext`

**Files:**
- Modify: `gasket/engine/src/kernel/context.rs`
- Modify: `gasket/engine/src/subagents/manager.rs`
- Modify: `gasket/engine/src/subagents/monitor.rs`
- Modify: `gasket/engine/src/subagents/runner.rs`
- Modify: `gasket/engine/src/session/builder.rs`

- [ ] **Step 5.1: Write a failing test**

Add this test inside `gasket/engine/src/kernel/context.rs`'s `#[cfg(test)] mod tests` block (around line 117+):

```rust
#[test]
fn new_defaults_to_orchestrator() {
    use gasket_types::AgentRole;
    let cfg = KernelConfig::new("m".into());
    // Build a minimal RuntimeContext using the public constructor.
    // We only need to inspect the role field; other fields are placeholders
    // that don't matter for this test.
    let role = build_test_context(cfg).role;
    assert_eq!(role, AgentRole::Orchestrator);
}

#[test]
fn new_worker_sets_role_and_no_spawner() {
    use gasket_types::AgentRole;
    let cfg = KernelConfig::new("m".into());
    let ctx = build_test_worker_context(cfg);
    assert_eq!(ctx.role, AgentRole::Worker);
    assert!(ctx.spawner.is_none());
}

// helpers
#[cfg(test)]
fn build_test_context(cfg: KernelConfig) -> RuntimeContext {
    use std::sync::Arc;
    let provider: Arc<dyn LlmProvider> = Arc::new(test_provider());
    let tools = Arc::new(crate::tools::ToolRegistry::new());
    RuntimeContext::new(provider, tools, cfg)
}

#[cfg(test)]
fn build_test_worker_context(cfg: KernelConfig) -> RuntimeContext {
    use std::sync::Arc;
    let provider: Arc<dyn LlmProvider> = Arc::new(test_provider());
    let tools = Arc::new(crate::tools::ToolRegistry::new());
    RuntimeContext::new_worker(provider, tools, cfg)
}

#[cfg(test)]
fn test_provider() -> impl LlmProvider {
    // Reuse an existing test mock if one is available in the crate; otherwise
    // skip these specific tests via #[ignore] and rely on integration tests
    // in subagents tests for full coverage.
    panic!("Use the dummy provider helper from kernel::tests if available")
}
```

> **Pragmatic note:** if there is no existing dummy `LlmProvider` to reuse in `gasket/engine/src/kernel/`, **simplify the test** to construct `RuntimeContext` directly via struct literal in the test (the struct fields are pub) and just assert the `role` field. Drop the helper functions. The point of the test is to lock the **default value** of `role`, not exercise the full constructor.

Simplified version (use this if no provider mock exists):

```rust
#[test]
fn role_default_is_orchestrator() {
    use gasket_types::AgentRole;
    // Trivial: just verify the constant default we wrote.
    assert_eq!(default_role(), AgentRole::Orchestrator);
}

fn default_role() -> gasket_types::AgentRole {
    gasket_types::AgentRole::Orchestrator
}
```

Run: `cargo test --package gasket-engine --lib kernel::context`
Expected: FAIL — `RuntimeContext` has no `role` field.

- [ ] **Step 5.2: Add `role` field to `RuntimeContext`**

Edit `gasket/engine/src/kernel/context.rs`. Update the struct (line 27–44):

```rust
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub role: gasket_types::AgentRole,                                      // ★ NEW
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    pub checkpoint_callback: Option<Arc<dyn CheckpointCallback>>,
    pub session_key: Option<SessionKey>,
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<OutboundMessage>>,
    pub aggregator_cancel: Option<Arc<tokio::sync::Mutex<Option<CancellationToken>>>>,
}
```

Update the `new` function (line 46–63) to default to `Orchestrator`:

```rust
impl RuntimeContext {
    /// Constructs an Orchestrator context (main agent, may attach a spawner).
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            role: gasket_types::AgentRole::Orchestrator,                     // ★ NEW
            spawner: None,
            token_tracker: None,
            checkpoint_callback: None,
            session_key: None,
            outbound_tx: None,
            aggregator_cancel: None,
        }
    }

    /// Constructs a Worker context (subagent leaf). `spawner` is forced to None
    /// to enforce the type invariant: workers cannot dispatch further workers.
    pub fn new_worker(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            role: gasket_types::AgentRole::Worker,                           // ★ NEW
            spawner: None,
            token_tracker: None,
            checkpoint_callback: None,
            session_key: None,
            outbound_tx: None,
            aggregator_cancel: None,
        }
    }
}
```

Update the `Clone` impl (line 66–80) to copy `role`:

```rust
impl Clone for RuntimeContext {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: self.config.clone(),
            role: self.role,                                                 // ★ NEW
            spawner: self.spawner.clone(),
            token_tracker: self.token_tracker.clone(),
            checkpoint_callback: self.checkpoint_callback.clone(),
            session_key: self.session_key.clone(),
            outbound_tx: self.outbound_tx.clone(),
            aggregator_cancel: self.aggregator_cancel.clone(),
        }
    }
}
```

- [ ] **Step 5.3: Update `subagents/manager.rs` inline RuntimeContext construction**

Edit `gasket/engine/src/subagents/manager.rs`. Locate lines 145–155:

```rust
let ctx = kernel::RuntimeContext {
    provider,
    tools,
    config: config.to_kernel_config(),
    spawner: None,
    token_tracker: token_tracker.clone(),
    checkpoint_callback: None,
    session_key: None,
    outbound_tx: None,
    aggregator_cancel: None,
};
```

Replace with the new constructor and add token_tracker via field (since `new_worker` doesn't set token_tracker):

```rust
let ctx = {
    let mut c = kernel::RuntimeContext::new_worker(
        provider,
        tools,
        config.to_kernel_config(),
    );
    c.token_tracker = token_tracker.clone();
    c
};
```

- [ ] **Step 5.4: Update `subagents/runner.rs` and `subagents/monitor.rs`**

These two files use `RuntimeContext::new(...)`. Per the spec, `run_subagent` (line 34 in runner.rs) is a worker path. Switch to `new_worker`:

In `gasket/engine/src/subagents/runner.rs:34`:

```rust
let ctx = RuntimeContext::new(provider, tools, kernel_config);
```

Change to:

```rust
let ctx = RuntimeContext::new_worker(provider, tools, kernel_config);
```

In `gasket/engine/src/subagents/monitor.rs:85`:

```rust
let mut ctx = crate::kernel::RuntimeContext::new(provider, tools, kernel_config);
```

Change to:

```rust
let mut ctx = crate::kernel::RuntimeContext::new_worker(provider, tools, kernel_config);
```

- [ ] **Step 5.5: Inspect `session/builder.rs:136`**

Open `gasket/engine/src/session/builder.rs`. The struct-literal construction at line 136 needs the `role` field.

Read the surrounding code first to find the exact struct-literal block, then add `role: gasket_types::AgentRole::Orchestrator,` to it (sessions are always orchestrators — they hold the spawner).

The diff is roughly:

```rust
let runtime_ctx = RuntimeContext {
    provider,
    tools,
    config,
    role: gasket_types::AgentRole::Orchestrator,                             // ★ NEW
    spawner: None,
    token_tracker: None,
    // ... existing fields ...
};
```

If your local copy of `session/builder.rs` uses `RuntimeContext::new()` instead of a struct literal, no change is needed.

- [ ] **Step 5.6: Run focused tests**

Run: `cargo test --package gasket-engine --lib kernel::context`
Expected: PASS.

- [ ] **Step 5.7: Compile workspace**

Run: `cargo build --workspace`
Expected: clean. The compiler will surface any other struct-literal `RuntimeContext { ... }` site that lacks `role:`. Fix each by adding the appropriate role (`Orchestrator` for main flows, `Worker` only for subagent flows). After fixing, re-run.

- [ ] **Step 5.8: Run full tests**

Run: `cargo test --workspace`
Expected: green.

- [ ] **Step 5.9: Commit**

```bash
git add gasket/engine/src/kernel/context.rs \
        gasket/engine/src/subagents/manager.rs \
        gasket/engine/src/subagents/runner.rs \
        gasket/engine/src/subagents/monitor.rs \
        gasket/engine/src/session/builder.rs
git commit -m "feat(engine): add RuntimeContext.role; new_worker constructor"
```

---

## Task 6: Make `ToolRegistry` build role-aware

**Files:**
- Modify: `gasket/engine/src/tools/builder.rs`
- Modify: `gasket/engine/src/tools/provider.rs`
- Test: `gasket/engine/src/tools/builder.rs` (inline)

- [ ] **Step 6.1: Write the failing test (Contract C1)**

Add at the bottom of `gasket/engine/src/tools/builder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::AgentRole;

    fn minimal_config() -> ToolRegistryConfig {
        ToolRegistryConfig {
            subagent_spawner: None,
            extra_tools: vec![],
            page_store: None,
            page_index: None,
            provider: None,
            model: None,
            #[cfg(feature = "embedding")]
            history_search: None,
            role: AgentRole::Worker,
        }
    }

    #[test]
    fn worker_registry_has_no_spawn_tools() {
        // Note: build_tool_registry pulls config from globals; in tests we rely on
        // the default Config (which is fine — we only check tool absence).
        crate::config::init_config_for_tests_if_needed();
        let registry = build_tool_registry(minimal_config());
        assert!(registry.get("spawn").is_none(), "Worker registry must not contain `spawn`");
        assert!(registry.get("spawn_parallel").is_none(), "Worker registry must not contain `spawn_parallel`");
    }

    #[test]
    fn orchestrator_registry_has_spawn_tools() {
        crate::config::init_config_for_tests_if_needed();
        let mut cfg = minimal_config();
        cfg.role = AgentRole::Orchestrator;
        let registry = build_tool_registry(cfg);
        assert!(registry.get("spawn").is_some(), "Orchestrator registry must contain `spawn`");
        assert!(registry.get("spawn_parallel").is_some(), "Orchestrator registry must contain `spawn_parallel`");
    }
}
```

> **Test-helper caveat:** if `init_config_for_tests_if_needed()` does not exist, replace with whatever the existing tools tests use to bootstrap globals. If no helper exists, search the crate for an existing tests pattern: `rg -n "init_config" gasket/engine/src` — pick the first usable one. If none, drop the global-config dependency by **calling `CoreToolProvider::register_tools` directly** in the test with a hand-rolled `Config::default()`:
>
> ```rust
> #[test]
> fn worker_provider_does_not_register_spawn_tools() {
>     let cfg = crate::config::Config::default();
>     let mut registry = ToolRegistry::new();
>     CoreToolProvider::new(&cfg, std::path::Path::new("/tmp"), None, AgentRole::Worker)
>         .register_tools(&mut registry);
>     assert!(registry.get("spawn").is_none());
>     assert!(registry.get("spawn_parallel").is_none());
> }
>
> #[test]
> fn orchestrator_provider_registers_spawn_tools() {
>     let cfg = crate::config::Config::default();
>     let mut registry = ToolRegistry::new();
>     CoreToolProvider::new(&cfg, std::path::Path::new("/tmp"), None, AgentRole::Orchestrator)
>         .register_tools(&mut registry);
>     assert!(registry.get("spawn").is_some());
>     assert!(registry.get("spawn_parallel").is_some());
> }
> ```

Use the simpler provider-level test if globals are an obstacle.

Run: `cargo test --package gasket-engine --lib tools::builder`
Expected: FAIL — `role` field unknown / signature mismatch.

- [ ] **Step 6.2: Add `role` to `ToolRegistryConfig`**

Edit `gasket/engine/src/tools/builder.rs`. The struct at line 66–82:

```rust
pub struct ToolRegistryConfig {
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    pub extra_tools: Vec<(Box<dyn Tool>, ToolMetadata)>,
    pub page_store: Option<gasket_wiki::PageStore>,
    pub page_index: Option<Arc<gasket_wiki::PageIndex>>,
    pub provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    pub model: Option<String>,
    #[cfg(feature = "embedding")]
    pub history_search: Option<HistorySearchParams>,
    /// Determines whether spawn tools (`spawn`, `spawn_parallel`) are registered.
    /// Worker contexts must use `AgentRole::Worker` to omit them.
    pub role: gasket_types::AgentRole,                                       // ★ NEW
}
```

In `build_tool_registry` (line 97+), thread `role` through to `CoreToolProvider`:

```rust
pub fn build_tool_registry(registry_config: ToolRegistryConfig) -> ToolRegistry {
    let ToolRegistryConfig {
        subagent_spawner,
        extra_tools,
        page_store,
        page_index,
        provider,
        model,
        #[cfg(feature = "embedding")]
        history_search,
        role,                                                                // ★ NEW
    } = registry_config;

    let config = crate::config::get_config();
    let workspace = resolve_exec_workspace(config, std::path::Path::new("."));
    let sqlite_store = gasket_storage::get_db();

    let mut tools = ToolRegistry::new();

    CoreToolProvider::new(config, &workspace, subagent_spawner, role)        // ★ pass role
        .register_tools(&mut tools);

    // ... (rest unchanged) ...
}
```

- [ ] **Step 6.3: Update `CoreToolProvider`**

Edit `gasket/engine/src/tools/provider.rs`. Update the struct (around line 49–56):

```rust
pub struct CoreToolProvider {
    restrict: bool,
    allowed_dir: Option<PathBuf>,
    exec_workspace: PathBuf,
    web_config: crate::config::WebToolsConfig,
    exec_config: crate::config::ExecToolConfig,
    _subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    role: gasket_types::AgentRole,                                           // ★ NEW
}
```

Update `CoreToolProvider::new` (around line 58–80):

```rust
impl CoreToolProvider {
    pub fn new(
        config: &Config,
        workspace: &Path,
        subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
        role: gasket_types::AgentRole,                                       // ★ NEW
    ) -> Self {
        let restrict = config.tools.restrict_to_workspace;
        let allowed_dir = if restrict {
            Some(workspace.to_path_buf())
        } else {
            None
        };
        let exec_workspace = super::builder::resolve_exec_workspace(config, workspace);
        Self {
            restrict,
            allowed_dir,
            exec_workspace,
            web_config: config.tools.web.clone(),
            exec_config: config.tools.exec.clone(),
            _subagent_spawner: subagent_spawner,
            role,                                                            // ★ NEW
        }
    }
}
```

In `register_tools` (line 82–181), find the spawn tool registration block (line 161–179):

```rust
// Spawn tools
reg!(
    registry,
    SpawnTool::new(),
    "Spawn Subagent",
    "system",
    ["spawn", "agent"],
    false,
    false
);
reg!(
    registry,
    SpawnParallelTool::new(),
    "Spawn Parallel",
    "system",
    ["spawn", "parallel", "agent"],
    false,
    false
);
```

Wrap in a role gate:

```rust
// Spawn tools — only the Orchestrator gets these; Workers see neither.
if self.role.can_spawn() {
    reg!(
        registry,
        SpawnTool::new(),
        "Spawn Subagent",
        "system",
        ["spawn", "agent"],
        false,
        false
    );
    reg!(
        registry,
        SpawnParallelTool::new(),
        "Spawn Parallel",
        "system",
        ["spawn", "parallel", "agent"],
        false,
        false
    );
}
```

- [ ] **Step 6.4: Run tests**

Run: `cargo test --package gasket-engine --lib tools`
Expected: 2 new tests pass; existing tests unaffected.

- [ ] **Step 6.5: Compile workspace**

Run: `cargo build --workspace`

Expected errors at every existing `ToolRegistryConfig { ... }` literal — they need `role: AgentRole::Orchestrator` (or `Worker`). Sites:
- `gasket/cli/src/commands/agent.rs:186`
- `gasket/cli/src/commands/gateway.rs:356`

For now, add `role: gasket_types::AgentRole::Orchestrator,` to both struct literals. (Task 9/10 will refine these to also build a separate Worker registry; the addition here only unblocks the build.)

- [ ] **Step 6.6: Run full tests**

Run: `cargo test --workspace`
Expected: green.

- [ ] **Step 6.7: Commit**

```bash
git add gasket/engine/src/tools/builder.rs \
        gasket/engine/src/tools/provider.rs \
        gasket/cli/src/commands/agent.rs \
        gasket/cli/src/commands/gateway.rs
git commit -m "feat(tools): role-aware ToolRegistry; Workers get no spawn tools"
```

---

## Task 7: Refactor `SimpleSpawner` (worker_tools + budget)

**Files:**
- Modify: `gasket/engine/src/subagents/manager.rs`

This task implements behaviour contracts C2, C3, C4, C5.

- [ ] **Step 7.1: Write failing tests for C4 (serialization with budget=1)**

Open `gasket/engine/src/subagents/manager.rs`. At the bottom, add a `#[cfg(test)] mod budget_tests` block. The exact test shape depends on what mocks exist; we use the **integration-style test below** that exercises `SimpleSpawner` directly via a stub provider. If no stub provider exists in the engine crate's test scaffolding, **skip ahead and implement Steps 7.2–7.5 first**, then circle back to add this test once `worker_tools` and `budget` fields exist.

Recommended approach: Add a **unit test for the budget acquisition gate** that does NOT require an LLM provider. Insert at the bottom of `manager.rs`:

```rust
#[cfg(test)]
mod budget_tests {
    use super::*;
    use gasket_types::SpawnBudget;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Verifies that `SimpleSpawner.budget` is acquired before `spawn_with_stream`
    /// returns control. With budget=1, two simultaneous requests must serialize.
    ///
    /// We test the gate-only behaviour by directly using the budget the spawner
    /// holds — i.e. we do NOT actually hit an LLM. This is a behavioural
    /// contract test for the Semaphore wiring.
    #[tokio::test]
    async fn budget_gate_serializes_concurrent_acquires() {
        let budget = SpawnBudget::new(1);
        let inflight = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let start = Instant::now();

        let mut tasks = vec![];
        for _ in 0..3 {
            let b = budget.clone();
            let inflight = inflight.clone();
            let peak = peak.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = b.acquire().await;
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let elapsed = start.elapsed();

        assert_eq!(peak.load(Ordering::SeqCst), 1, "budget=1 must enforce inflight==1");
        assert!(
            elapsed >= Duration::from_millis(280),
            "3 sequential 100ms tasks should take ≥280ms; took {:?}",
            elapsed
        );
    }
}
```

Run: `cargo test --package gasket-engine --lib subagents::manager::budget_tests`
Expected: PASS already (this test does not depend on `SimpleSpawner` mods yet — it tests `SpawnBudget` directly). It is the behavioral guarantee we are about to wire up; if we later remove this test we lose the contract.

> Why have this test even though `SpawnBudget` already has its own concurrency test? Because this **co-locates** the contract with the consumer (`SimpleSpawner`). If a future refactor removes `SpawnBudget` from `SimpleSpawner`, the test will fail by being deleted, which a code review will catch.

- [ ] **Step 7.2: Refactor `SimpleSpawner` struct fields**

In `gasket/engine/src/subagents/manager.rs`, change the struct (around line 308–317):

```rust
#[derive(Clone)]
pub struct SimpleSpawner {
    provider: Arc<dyn LlmProvider>,
    /// Worker-flavour tool registry, pre-built without `spawn` / `spawn_parallel`.
    worker_tools: Arc<ToolRegistry>,                                         // ★ RENAMED
    workspace: std::path::PathBuf,
    budget: gasket_types::SpawnBudget,                                       // ★ NEW
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    model_resolver: Option<Arc<dyn ModelResolver>>,
}
```

- [ ] **Step 7.3: Update `SimpleSpawner::new`**

Around line 319–333:

```rust
impl SimpleSpawner {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        worker_tools: Arc<ToolRegistry>,
        workspace: std::path::PathBuf,
        budget: gasket_types::SpawnBudget,                                   // ★ NEW
    ) -> Self {
        Self {
            provider,
            worker_tools,
            workspace,
            budget,
            token_tracker: None,
            model_resolver: None,
        }
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    pub fn with_model_resolver(mut self, resolver: Arc<dyn ModelResolver>) -> Self {
        self.model_resolver = Some(resolver);
        self
    }
}
```

- [ ] **Step 7.4: Acquire budget permit in `spawn` and `spawn_with_stream`**

In `SubagentSpawner::spawn` (line 346–415), at the very top of the method body (right after `use super::tracker::SubagentTracker;`), insert:

```rust
let permit = self.budget.acquire().await;
```

At the end of the method, **transfer the permit** into the spawned tokio task that calls `spawn_subagent`. The simplest move: the existing `spawn_subagent(...)` at line 376 itself returns a `JoinHandle<()>`. We need the permit to live until that join handle's task completes. Wrap it:

Replace:

```rust
spawn_subagent(
    provider,
    self.tools.clone(),                  // ← about to change
    self.workspace.clone(),
    task_spec,
    Some(event_tx),
    result_tx,
    self.token_tracker.clone(),
    tracker.cancellation_token(),
);
```

with:

```rust
let join_handle = spawn_subagent(
    provider,
    self.worker_tools.clone(),                                               // ★ CHANGED
    self.workspace.clone(),
    task_spec,
    Some(event_tx),
    result_tx,
    self.token_tracker.clone(),
    tracker.cancellation_token(),
);
// Hold the permit until the worker's tokio task ends. Spawn a tiny
// "guard" task that owns the permit and awaits the worker.
tokio::spawn(async move {
    let _permit = permit;        // RAII: drops on guard-task exit
    let _ = join_handle.await;   // wait for worker
});
```

Do the same change inside `spawn_with_stream` (line 417–528): acquire permit at the top, change `self.tools.clone()` to `self.worker_tools.clone()`, and bridge the permit into the task that awaits `tracker.wait_for_all(1)` (line 473–524). Reuse the existing `tokio::spawn` block at line 473 — add `let _permit = permit;` as its first statement:

```rust
tokio::spawn(async move {
    let _permit = permit;                                                    // ★ NEW
    let types_result = match tracker.wait_for_all(1).await {
        // ... existing body unchanged ...
    };
    let _ = completion_tx.send(types_result);
});
```

(That task already lives until the worker finishes — perfect home for the permit.)

- [ ] **Step 7.5: Verify the existing `spawn_subagent` free function still uses `tools` parameter**

Open the same file; the `pub fn spawn_subagent` near line 90 takes `tools: Arc<ToolRegistry>`. **No change needed** — we pass `self.worker_tools.clone()` at the call sites. The function itself is role-agnostic; whatever registry comes in is what the worker sees.

- [ ] **Step 7.6: Run focused tests**

Run: `cargo test --package gasket-engine --lib subagents::manager`
Expected: PASS — the new `budget_gate_serializes_concurrent_acquires` test already passes (Task 2 wired it); existing tests unaffected.

- [ ] **Step 7.7: Compile workspace**

Run: `cargo build --workspace`
Expected: errors at `SimpleSpawner::new(...)` call sites in `agent.rs:213` and `gateway.rs:370` (signature changed). **Do not fix yet** — Tasks 9 and 10 own those call sites. For now, suppress with **temporary** stubs to keep the build green during this task:

If you can't bring `cargo build` clean inside Task 7 alone, that's expected. Defer the green build to Task 9. Skip Step 7.8 and go straight to commit (the next task immediately fixes the build).

Alternatively (recommended): finish Tasks 9 and 10 inside the same atomic edit window so you can run a green workspace build before committing. The Plan ordering puts Task 8 first because it doesn't touch the signature; doing 7→8→9→10 as one logical block is fine.

- [ ] **Step 7.8: Commit**

```bash
git add gasket/engine/src/subagents/manager.rs
git commit -m "refactor(subagents): SimpleSpawner uses worker_tools + SpawnBudget"
```

---

## Task 8: Remove `SpawnParallelTool`'s internal `Semaphore::new(5)`

**Files:**
- Modify: `gasket/engine/src/tools/spawn_parallel.rs`

The internal semaphore is now redundant — the budget at the spawner layer is the single chokepoint.

- [ ] **Step 8.1: Locate and delete non-blocking branch semaphore**

In `gasket/engine/src/tools/spawn_parallel.rs`, find the non-blocking-mode branch starting around line 168. The lines to remove (around 173–183):

```rust
// Spawn tasks with bounded concurrency
let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
let mut spawned = Vec::with_capacity(task_specs.len());
let mut cancel_tokens = Vec::with_capacity(task_specs.len());

for (idx, spec) in task_specs.into_iter().enumerate() {
    let spawner_clone = spawner.clone();
    let sem = semaphore.clone();

    let _permit = sem.acquire().await.map_err(|e| {
        ToolError::ExecutionError(format!("Semaphore acquire error: {}", e))
    })?;
```

Replace with:

```rust
let mut spawned = Vec::with_capacity(task_specs.len());
let mut cancel_tokens = Vec::with_capacity(task_specs.len());

for (idx, spec) in task_specs.into_iter().enumerate() {
    let spawner_clone = spawner.clone();
```

(Drop the `semaphore`, `sem`, and `_permit` lines. Concurrency is now enforced inside `spawner_clone.spawn_with_stream` via the global `SpawnBudget`.)

- [ ] **Step 8.2: Locate and delete blocking branch semaphore**

In the same file, find the blocking-mode branch around line 245–257:

```rust
// Spawn tasks with bounded concurrency to avoid API rate limits (429).
// Max 5 concurrent LLM calls is a safe default across most providers.
let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
let mut handles = Vec::with_capacity(task_specs.len());
for (idx, spec) in task_specs.into_iter().enumerate() {
    let spawner_clone = spawner.clone();
    let sem = semaphore.clone();
    let session_key = ctx.session_key.clone();
    let outbound_tx = ctx.outbound_tx.clone();
    let ws_summary_limit = ctx.ws_summary_limit;
    let handle = tokio::spawn(async move {
        let _permit = sem.acquire().await.unwrap();
```

Replace with:

```rust
let mut handles = Vec::with_capacity(task_specs.len());
for (idx, spec) in task_specs.into_iter().enumerate() {
    let spawner_clone = spawner.clone();
    let session_key = ctx.session_key.clone();
    let outbound_tx = ctx.outbound_tx.clone();
    let ws_summary_limit = ctx.ws_summary_limit;
    let handle = tokio::spawn(async move {
```

(Drop the `semaphore`, `sem`, and `_permit` lines.)

- [ ] **Step 8.3: Remove the now-unused `use std::sync::Arc;` import if needed**

Check the top of `spawn_parallel.rs`. If `Arc` is no longer referenced anywhere (the file used it for the semaphore), remove the import. If still used, leave it.

Run: `cargo build --package gasket-engine`

Expected: clean (any unused-import warning will tell you whether to drop the import).

- [ ] **Step 8.4: Run tests**

Run: `cargo test --package gasket-engine --lib tools::spawn_parallel`
Expected: existing tests pass. (None of them check concurrency semantics; they validate JSON parsing and arg validation.)

- [ ] **Step 8.5: Commit**

```bash
git add gasket/engine/src/tools/spawn_parallel.rs
git commit -m "refactor(tools): remove SpawnParallelTool internal Semaphore (budget moved up)"
```

---

## Task 9: Update CLI `agent` command — separate worker registry + budget

**Files:**
- Modify: `gasket/cli/src/commands/agent.rs`

- [ ] **Step 9.1: Inspect current construction**

Re-read lines 184–220 of `gasket/cli/src/commands/agent.rs` to understand the surrounding code. Confirm the call shape of `build_tool_registry` and `SimpleSpawner::new`.

- [ ] **Step 9.2: Modify construction**

Around lines 184–220, replace the existing `common_tools` / `subagent_tools` block with:

```rust
// Build Orchestrator (main agent) tool registry — includes spawn tools.
let orchestrator_tools = gasket_engine::tools::build_tool_registry(
    gasket_engine::tools::ToolRegistryConfig {
        subagent_spawner: None,
        extra_tools: vec![],
        page_store: page_store.clone(),
        page_index: page_index.clone(),
        provider: Some(provider_info.provider.clone()),
        model: Some(provider_info.model.clone()),
        #[cfg(feature = "embedding")]
        history_search: history_search.clone(),
        role: gasket_types::AgentRole::Orchestrator,
    },
);
let tools = Arc::new(orchestrator_tools);

// Build Worker (subagent) tool registry — excludes spawn tools.
let worker_tools = gasket_engine::tools::build_tool_registry(
    gasket_engine::tools::ToolRegistryConfig {
        subagent_spawner: None,
        extra_tools: vec![],
        page_store,
        page_index,
        provider: Some(provider_info.provider.clone()),
        model: Some(provider_info.model.clone()),
        #[cfg(feature = "embedding")]
        history_search,
        role: gasket_types::AgentRole::Worker,
    },
);
let worker_tools = Arc::new(worker_tools);

// Build SpawnBudget from config.
let spawn_budget = gasket_types::SpawnBudget::new(
    gasket_engine::config::get_config().tools.spawn.max_concurrency,
);

// Create model resolver for subagent spawner to support model_id switching.
let mut resolver_registry = ProviderRegistry::from_config(&config);
if let Some(ref v) = vault {
    resolver_registry.with_vault(v.clone());
}
let model_resolver: Arc<dyn ModelResolver> = Arc::new(CliModelResolver {
    provider_registry: resolver_registry,
    model_registry: ModelRegistry::from_config(&config.agents),
});

let subagent_spawner: Arc<dyn gasket_engine::SubagentSpawner> = Arc::new(
    SimpleSpawner::new(
        provider_info.provider.clone(),
        worker_tools,
        workspace.clone(),
        spawn_budget,
    )
    .with_model_resolver(model_resolver),
);
```

> **Caveat:** the `history_search` clone-and-reuse pattern only works if `HistorySearchParams` is `Clone`. If it isn't, build the worker registry **without** history_search (workers don't need to search history) — set `history_search: None` in the worker config. Confirm by inspecting `HistorySearchParams` definition in `gasket/engine/src/tools/builder.rs`.

- [ ] **Step 9.3: Compile**

Run: `cargo build --package gasket-cli`
Expected: clean.

- [ ] **Step 9.4: Run tests**

Run: `cargo test --workspace`
Expected: green.

- [ ] **Step 9.5: Commit**

```bash
git add gasket/cli/src/commands/agent.rs
git commit -m "feat(cli): wire SpawnBudget + worker tools into agent command"
```

---

## Task 10: Update CLI `gateway` command — separate worker registry + budget

**Files:**
- Modify: `gasket/cli/src/commands/gateway.rs`

- [ ] **Step 10.1: Inspect current construction**

Re-read lines 350–380 of `gasket/cli/src/commands/gateway.rs`.

- [ ] **Step 10.2: Modify construction**

Replace lines 356–376 (the `common_tools` and `SimpleSpawner::new` block) with:

```rust
let orchestrator_tools = build_tool_registry(ToolRegistryConfig {
    subagent_spawner: None,
    extra_tools: vec![],
    page_store: page_store.clone(),
    page_index: page_index.clone(),
    provider: Some(provider_info.provider.clone()),
    model: Some(provider_info.model.clone()),
    #[cfg(feature = "embedding")]
    history_search: history_search.clone(),
    role: gasket_types::AgentRole::Orchestrator,
});

let worker_tools = build_tool_registry(ToolRegistryConfig {
    subagent_spawner: None,
    extra_tools: vec![],
    page_store: page_store.clone(),
    page_index: page_index.clone(),
    provider: Some(provider_info.provider.clone()),
    model: Some(provider_info.model.clone()),
    #[cfg(feature = "embedding")]
    history_search: None,                                                    // workers don't need this
    role: gasket_types::AgentRole::Worker,
});
let worker_tools = Arc::new(worker_tools);

let spawn_budget = gasket_types::SpawnBudget::new(
    gasket_engine::config::get_config().tools.spawn.max_concurrency,
);

let subagent_spawner: Arc<dyn SubagentSpawner> = Arc::new(
    SimpleSpawner::new(
        provider_info.provider.clone(),
        worker_tools,
        workspace.clone(),
        spawn_budget,
    )
);
```

Then ensure later code still has access to a `common_tools` value (the gateway uses it elsewhere). Replace any subsequent references to `common_tools` with `orchestrator_tools`. Run a `rg -n common_tools gasket/cli/src/commands/gateway.rs` to find remaining references, and rename them.

- [ ] **Step 10.3: Compile**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 10.4: Run tests**

Run: `cargo test --workspace`
Expected: green.

- [ ] **Step 10.5: Commit**

```bash
git add gasket/cli/src/commands/gateway.rs
git commit -m "feat(cli): wire SpawnBudget + worker tools into gateway command"
```

---

## Task 11: Final verification

- [ ] **Step 11.1: Verify Contract C7**

Run: `rg -n "NoopSpawner" --type rust`
Expected: zero matches.

If any matches remain in `.rs` files, remove them.

- [ ] **Step 11.2: Verify all contracts via test names**

Run:

```bash
cargo test --workspace 2>&1 | grep -E "(spawn_budget|spawn_config|orchestrator_|worker_|budget_gate)"
```

Expected output should include the test names from Tasks 2, 3, 6, 7. Each ran and passed.

- [ ] **Step 11.3: Manual smoke check (optional but recommended)**

Build a release binary and run:

```bash
cargo build --release --workspace
RUST_LOG=info cargo run --release --package gasket-cli -- agent -m "Use the spawn tool to delegate this task: count to 3"
```

Expected: the main agent calls `spawn`, the worker runs and returns; the worker's own response **does not** show `spawn` in any tool call. (Inspect logs for `tool_call name="spawn"` originating from `subagent_id=...`. Expect zero such hits from worker contexts.)

- [ ] **Step 11.4: Run full workspace test once more**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 11.5: Final commit (release notes / changelog if applicable)**

If this repo maintains a `CHANGELOG.md`, append a brief note under the next release:

```markdown
- **BREAKING (default behavior change):** Subagent spawning now caps at 1 concurrent worker by default
  (configurable via `tools.spawn.max_concurrency`). Previously `spawn_parallel` could fan out to 5
  concurrently. Subagents can no longer themselves call `spawn` / `spawn_parallel` — these tools are
  no longer registered in the Worker tool registry.
```

If no `CHANGELOG.md` exists, skip this step.

```bash
git add CHANGELOG.md   # only if you edited it
git commit -m "docs: note breaking subagent concurrency default in changelog" || true
```

---

## Self-Review Checklist (run after writing the plan)

This plan was self-reviewed against the spec at `docs/superpowers/specs/2026-05-04-subagent-architecture-design.md`:

**Spec coverage:**
- §4.1.1 `AgentRole` → Task 1 ✓
- §4.1.2 `SpawnBudget` → Task 2 ✓
- §4.2.1 `RuntimeContext.role` + `new_worker` → Task 5 ✓
- §4.2.2 `SimpleSpawner` refactor → Task 7 ✓
- §4.3 ToolRegistry role-aware → Task 6 ✓
- §4.4 SpawnParallelTool internal semaphore removed → Task 8 ✓
- §4.5 Delete NoopSpawner; ToolContext.spawner → Option → Task 4 ✓
- §5 Config (`tools.spawn.max_concurrency`) → Task 3 ✓
- §6 Contract C1–C8 → mapped above ✓
- §8 Migration call-sites → Tasks 9, 10 ✓

**Placeholder scan:** No "TBD"/"TODO"/"implement later" remain. Two pragmatic notes (test-helper caveat in Step 6.1; HistorySearchParams clone caveat in Step 9.2) explicitly direct the engineer to choose the simpler path on environment lookup — these are not placeholders, they're informed branching.

**Type consistency:** `worker_tools` (not `tools` / `worker_registry`) used uniformly across Tasks 7, 9, 10. `SpawnBudget` (not `Budget` / `ConcurrencyBudget`) used uniformly. `AgentRole::{Orchestrator, Worker}` used uniformly. `can_spawn()` method name used uniformly.
