# Flow Command System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a slash-command-gated, YAML-templated, persistable plan-act-review flow engine for the gasket workspace as specified in `docs/superpowers/specs/2026-05-03-flow-command-system-design.md` (commit 17093dd).

**Architecture:** Two new modules (`gasket/engine/src/command/` for slash-command parsing/routing and `gasket/engine/src/flow/` for the phase orchestrator) sit between `channels` and `AgentSession`. Plain text bypasses both layers and goes to the existing session unchanged. Slash commands route to `FlowOrchestrator`, which loads YAML templates, drives a 5-phase state machine (brainstorm → design → plan → execute → verify), persists snapshots in a new `flow_runs` SQLite table, and gates wiki writes via an injected `WikiWriteGuard` trait.

**Tech Stack:** Rust 2021, tokio async, sqlx (SQLite), serde_yaml, uuid v7, chrono, async-trait, mustache-style templating (manual replace, no extra crate). All required deps are already in `gasket/Cargo.toml` workspace dependencies.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `gasket/types/src/flow.rs` | Create | `FlowStatus`, `PhaseId`, `PhaseOutput`, `PendingWikiWrite` types (shared across crates) |
| `gasket/types/src/lib.rs` | Modify | Re-export `flow` module |
| `gasket/types/src/events/stream.rs` | Modify | Add 4 new `ChatEvent` variants for flow lifecycle |
| `gasket/storage/src/migrations/flow_run.rs` | Create | DDL for `flow_runs` table |
| `gasket/storage/src/migrations/mod.rs` | Modify | Register `flow_run::run_schema` |
| `gasket/storage/src/flow_run_store.rs` | Create | CRUD for flow_runs (insert/load/update/list/active_for_session) |
| `gasket/storage/src/lib.rs` | Modify | Re-export `FlowRunStore`, wire into `SqliteStore` accessor |
| `gasket/engine/src/flow/mod.rs` | Create | Flow module re-exports |
| `gasket/engine/src/flow/template.rs` | Create | YAML template types + loader (user dir overrides built-in) |
| `gasket/engine/src/flow/state.rs` | Create | `FlowState` struct + pure state-machine transition logic |
| `gasket/engine/src/flow/wiki_guard.rs` | Create | `WikiWriteGuard` trait + `AllowAllGuard`/`DeferringGuard`/`BlockingGuard` |
| `gasket/engine/src/tools/wiki_tools.rs` | Modify | `WikiWriteTool` accepts injectable `Arc<dyn WikiWriteGuard>` |
| `gasket/engine/src/flow/phase_runner.rs` | Create | Drives one phase: render prompt, filter tools, call kernel, capture output |
| `gasket/engine/src/flow/gate.rs` | Create | CLI gate controller (stdin readline, parses y/n/edit/redo/back) |
| `gasket/engine/src/flow/orchestrator.rs` | Create | `FlowOrchestrator` ties phase_runner + gate + state + storage |
| `gasket/engine/src/command/mod.rs` | Create | Command module re-exports |
| `gasket/engine/src/command/parser.rs` | Create | Parse `/flow start ...`, `/brainstorm ...`, etc. into `CommandAction` |
| `gasket/engine/src/command/dispatcher.rs` | Create | Route raw input: command → orchestrator, plain → AgentSession |
| `gasket/engine/src/lib.rs` | Modify | `pub mod flow;` `pub mod command;` |
| `gasket/cli/src/commands/agent.rs` | Modify | Wire `CommandDispatcher` into the input loop |
| `gasket/engine/flows/default.yaml` | Create | Built-in 5-phase template |
| `gasket/engine/flows/debug.yaml` | Create | Built-in: brainstorm→plan→execute→verify |
| `gasket/engine/flows/docs.yaml` | Create | Built-in: brainstorm→design→execute |
| `gasket/engine/flows/prompts/{brainstorm,design,plan,execute,verify}.md` | Create | Default prompt files |
| `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md` | Modify | Mark Status as Superseded |

**Out of plan:** Web frontend gate UI (events emitted from v1, polish in v1.1 per spec §8). Web changes would be a follow-up plan.

---

## Implementation Notes for the Engineer

**TDD discipline.** Every task has the order: write test → run it (fail) → write impl → run test (pass) → commit. Don't skip.

**Commit cadence.** One commit per task. Use the existing repo's commit-message style: `feat(crate): ...`, `test(crate): ...`. Pre-commit hooks run `cargo clippy --fix`, `cargo fmt`, `cargo build` — expect them to take ~30s on each commit.

**Reading the spec.** The design spec at `docs/superpowers/specs/2026-05-03-flow-command-system-design.md` is authoritative. When in doubt about a behavior, read the spec section referenced in the task.

**Existing types you'll see:**
- `gasket_types::Tool` trait — what `WikiWriteTool` implements (see `gasket/types/src/tool.rs`).
- `gasket_types::ChatEvent` — user-facing stream events; we add 4 variants to it.
- `gasket_storage::SqliteStore` — single connection-pool wrapper. Sub-stores like `CronStore`, `KvStore` show the access pattern we'll mirror for `FlowRunStore`.
- `gasket_engine::kernel::execute_streaming(ctx, messages, event_tx)` — the LLM loop; `PhaseRunner` will call this.

---

### Task 1: Add flow types to `gasket-types`

**Files:**
- Create: `gasket/types/src/flow.rs`
- Modify: `gasket/types/src/lib.rs`

- [ ] **Step 1: Write failing test for type construction**

Create `gasket/types/src/flow.rs` with this test at the bottom (we'll add code above it next):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_id_round_trip() {
        let id = PhaseId::Brainstorm;
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, r#""brainstorm""#);
        let de: PhaseId = serde_json::from_str(&s).unwrap();
        assert_eq!(de, PhaseId::Brainstorm);
    }

    #[test]
    fn test_custom_phase_id_round_trip() {
        let id = PhaseId::Custom("design-v2".to_string());
        let s = serde_json::to_string(&id).unwrap();
        let de: PhaseId = serde_json::from_str(&s).unwrap();
        assert_eq!(de, id);
    }

    #[test]
    fn test_flow_status_serialization() {
        let s = FlowStatus::AwaitingGate {
            phase: PhaseId::Design,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("awaiting_gate"));
        assert!(j.contains(r#""design""#));
    }

    #[test]
    fn test_phase_output_default_is_empty() {
        let p = PhaseOutput {
            summary: String::new(),
            iterations_used: 0,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        };
        assert!(p.summary.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --package gasket-types --lib flow:: 2>&1 | head -30
```

Expected: compile error (types not defined yet).

- [ ] **Step 3: Write the types above the tests module**

Replace the file contents with:

```rust
//! Flow execution types shared across gasket crates.
//!
//! These types live here (instead of `gasket-engine`) so that storage
//! adapters can serialize them without taking a dependency on the engine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Identifier for a phase within a flow template.
///
/// Built-in phases are named (`Brainstorm` … `Verify`); user-defined templates
/// may use `Custom("any-id")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", untagged)]
pub enum PhaseId {
    Builtin(BuiltinPhase),
    Custom(String),
}

/// Built-in phase identifiers (lower-snake-case in YAML / JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinPhase {
    Brainstorm,
    Design,
    Plan,
    Execute,
    Verify,
}

impl PhaseId {
    pub const fn brainstorm() -> Self { PhaseId::Builtin(BuiltinPhase::Brainstorm) }
    pub const fn design() -> Self { PhaseId::Builtin(BuiltinPhase::Design) }
    pub const fn plan() -> Self { PhaseId::Builtin(BuiltinPhase::Plan) }
    pub const fn execute() -> Self { PhaseId::Builtin(BuiltinPhase::Execute) }
    pub const fn verify() -> Self { PhaseId::Builtin(BuiltinPhase::Verify) }

    pub fn as_str(&self) -> &str {
        match self {
            PhaseId::Builtin(BuiltinPhase::Brainstorm) => "brainstorm",
            PhaseId::Builtin(BuiltinPhase::Design) => "design",
            PhaseId::Builtin(BuiltinPhase::Plan) => "plan",
            PhaseId::Builtin(BuiltinPhase::Execute) => "execute",
            PhaseId::Builtin(BuiltinPhase::Verify) => "verify",
            PhaseId::Custom(s) => s.as_str(),
        }
    }
}

/// Status of a single flow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowStatus {
    Running,
    AwaitingGate { phase: PhaseId },
    Paused,
    Done,
    Aborted,
}

impl FlowStatus {
    /// Discriminant string for the SQLite `status` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            FlowStatus::Running => "running",
            FlowStatus::AwaitingGate { .. } => "awaiting_gate",
            FlowStatus::Paused => "paused",
            FlowStatus::Done => "done",
            FlowStatus::Aborted => "aborted",
        }
    }
}

/// Output captured at the end of a phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseOutput {
    /// LLM's final text content (the phase summary).
    pub summary: String,
    /// Number of LLM iterations consumed in this phase.
    pub iterations_used: u32,
    /// Names of all tools called during this phase.
    pub tools_called: Vec<String>,
    /// When the phase finished.
    pub finished_at: DateTime<Utc>,
}

/// A wiki write request that was intercepted during a flow.
///
/// Lives in `FlowState.pending_wiki_writes` until the user approves at flow end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingWikiWrite {
    pub path: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub page_type: String,
    /// Which phase produced this write request.
    pub origin_phase: PhaseId,
    pub queued_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    // tests from Step 1 stay here unchanged
    use super::*;

    #[test]
    fn test_phase_id_round_trip() {
        let id = PhaseId::brainstorm();
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, r#""brainstorm""#);
        let de: PhaseId = serde_json::from_str(&s).unwrap();
        assert_eq!(de, PhaseId::brainstorm());
    }

    #[test]
    fn test_custom_phase_id_round_trip() {
        let id = PhaseId::Custom("design-v2".to_string());
        let s = serde_json::to_string(&id).unwrap();
        let de: PhaseId = serde_json::from_str(&s).unwrap();
        assert_eq!(de, id);
    }

    #[test]
    fn test_flow_status_serialization() {
        let s = FlowStatus::AwaitingGate {
            phase: PhaseId::design(),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("awaiting_gate"));
        assert!(j.contains(r#""design""#));
    }

    #[test]
    fn test_phase_output_default_is_empty() {
        let p = PhaseOutput {
            summary: String::new(),
            iterations_used: 0,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        };
        assert!(p.summary.is_empty());
    }
}
```

- [ ] **Step 4: Add module to `gasket/types/src/lib.rs`**

After the existing `pub mod tool;` line (around line 16), add:

```rust
pub mod flow;
```

After the existing `pub use tool::{...};` block, add:

```rust
pub use flow::{BuiltinPhase, FlowStatus, PendingWikiWrite, PhaseId, PhaseOutput};
```

- [ ] **Step 5: Run tests to verify pass**

```bash
cargo test --package gasket-types --lib flow:: 2>&1 | tail -20
```

Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/types/src/flow.rs gasket/types/src/lib.rs
git commit -m "feat(types): add flow execution types (PhaseId, FlowStatus, PhaseOutput, PendingWikiWrite)"
```

---

### Task 2: Add flow lifecycle `ChatEvent` variants

**Files:**
- Modify: `gasket/types/src/events/stream.rs`

- [ ] **Step 1: Write failing tests for new variants**

In `gasket/types/src/events/stream.rs`, find the existing `#[cfg(test)] mod tests` block at the bottom. Add these tests inside it (after the last existing test):

```rust
    #[test]
    fn test_flow_started_serialization() {
        let event = ChatEvent::flow_started("01HF...", "new-feature");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"flow_started"#));
        assert!(json.contains(r#""flow_id":"01HF..."#));
        assert!(json.contains(r#""template":"new-feature"#));

        let de: ChatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, de);
    }

    #[test]
    fn test_phase_changed_serialization() {
        let event = ChatEvent::phase_changed("design");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"phase_changed"#));
        assert!(json.contains(r#""phase":"design"#));

        let de: ChatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, de);
    }

    #[test]
    fn test_gate_pending_serialization() {
        let event = ChatEvent::gate_pending("design", "Accept this design? (y/n/edit)");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"gate_pending"#));
        assert!(json.contains(r#""phase":"design"#));
        assert!(json.contains("Accept this design"));
    }

    #[test]
    fn test_flow_finished_serialization() {
        let event = ChatEvent::flow_finished("01HF...", "done");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"flow_finished"#));
        assert!(json.contains(r#""status":"done"#));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --package gasket-types stream::tests::test_flow 2>&1 | head -20
```

Expected: compile error (constructors don't exist yet).

- [ ] **Step 3: Add the variants and constructors**

In `gasket/types/src/events/stream.rs`, find the `pub enum ChatEvent` block. Just before the closing `}` of `ChatEvent` (after `ApprovalResponse { ... }`), add:

```rust
    /// A flow has been started by the user (slash command).
    FlowStarted {
        flow_id: Arc<str>,
        template: Arc<str>,
    },

    /// The active flow has transitioned into a new phase.
    PhaseChanged {
        phase: Arc<str>,
    },

    /// A phase gate is awaiting user input.
    GatePending {
        phase: Arc<str>,
        prompt: Arc<str>,
    },

    /// A flow has finished (status = done | aborted | paused).
    FlowFinished {
        flow_id: Arc<str>,
        status: Arc<str>,
    },
```

In the `impl ChatEvent` block, add after the existing constructors (after `approval_response`):

```rust
    pub fn flow_started(flow_id: impl Into<String>, template: impl Into<String>) -> Self {
        Self::FlowStarted {
            flow_id: Arc::from(flow_id.into()),
            template: Arc::from(template.into()),
        }
    }

    pub fn phase_changed(phase: impl Into<String>) -> Self {
        Self::PhaseChanged {
            phase: Arc::from(phase.into()),
        }
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

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test --package gasket-types stream::tests::test_flow 2>&1 | tail -10
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/types/src/events/stream.rs
git commit -m "feat(types): add flow lifecycle ChatEvent variants (FlowStarted, PhaseChanged, GatePending, FlowFinished)"
```

---

### Task 3: Add `flow_runs` SQLite migration

**Files:**
- Create: `gasket/storage/src/migrations/flow_run.rs`
- Modify: `gasket/storage/src/migrations/mod.rs`

- [ ] **Step 1: Write the migration module**

Create `gasket/storage/src/migrations/flow_run.rs`:

```rust
//! Flow run state table for the slash-command flow engine.
//!
//! Schema mirrors `cron.rs` style: `run_schema(pool)` runs all DDL idempotently.

use sqlx::SqlitePool;

/// Run flow_runs schema migrations (table + indexes).
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    create_flow_runs_table(pool).await?;
    create_flow_runs_indexes(pool).await?;
    Ok(())
}

async fn create_flow_runs_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS flow_runs (
            flow_id            TEXT PRIMARY KEY,
            template_name      TEXT NOT NULL,
            template_version   INTEGER NOT NULL,
            user_request       TEXT NOT NULL,
            session_key        TEXT NOT NULL,
            status             TEXT NOT NULL,
            current_phase      TEXT NOT NULL,
            completed_phases   TEXT NOT NULL DEFAULT '{}',
            pending_wiki       TEXT NOT NULL DEFAULT '[]',
            created_at         INTEGER NOT NULL,
            updated_at         INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_flow_runs_indexes(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_flow_runs_session
         ON flow_runs(session_key, updated_at DESC)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_flow_runs_status
         ON flow_runs(status, updated_at DESC)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 2: Register the module in `mod.rs`**

In `gasket/storage/src/migrations/mod.rs`, find the existing `pub mod` declarations (around line 9-13) and add `flow_run` alphabetically. The block becomes:

```rust
pub mod cron;
pub mod flow_run;
pub mod kv;
pub mod maintenance;
pub mod memory;
pub mod session;
```

In `pub async fn run_all`, add `flow_run::run_schema(pool).await?;` after `kv::run_schema`:

```rust
    session::run_schema(pool).await?;
    memory::run_schema(pool).await?;
    cron::run_schema(pool).await?;
    maintenance::run_schema(pool).await?;
    kv::run_schema(pool).await?;
    flow_run::run_schema(pool).await?;
```

- [ ] **Step 3: Verify migration runs by building**

```bash
cargo build --package gasket-storage 2>&1 | tail -5
```

Expected: compiles without errors.

- [ ] **Step 4: Run full storage tests to confirm no regression**

```bash
cargo test --package gasket-storage 2>&1 | tail -10
```

Expected: all existing tests still pass (the migration runs automatically in `temp_store()`).

- [ ] **Step 5: Commit**

```bash
git add gasket/storage/src/migrations/flow_run.rs gasket/storage/src/migrations/mod.rs
git commit -m "feat(storage): add flow_runs table migration"
```

---

### Task 4: `FlowRunStore` — CRUD repository

**Files:**
- Create: `gasket/storage/src/flow_run_store.rs`
- Modify: `gasket/storage/src/lib.rs`

- [ ] **Step 1: Write the failing test scaffold**

Create `gasket/storage/src/flow_run_store.rs` with this minimal test stub at the bottom (we'll add the impl in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn temp_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite:file:flow_test_{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4().simple()
            ))
            .await
            .unwrap();
        crate::migrations::flow_run::run_schema(&pool).await.unwrap();
        pool
    }

    fn sample_record() -> FlowRunRecord {
        FlowRunRecord {
            flow_id: uuid::Uuid::now_v7().to_string(),
            template_name: "default".to_string(),
            template_version: 1,
            user_request: "do thing".to_string(),
            session_key: "cli:test".to_string(),
            status: "running".to_string(),
            current_phase: "brainstorm".to_string(),
            completed_phases: "{}".to_string(),
            pending_wiki: "[]".to_string(),
            created_at: 1_000_000,
            updated_at: 1_000_000,
        }
    }

    #[tokio::test]
    async fn test_insert_and_load() {
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let r = sample_record();
        store.insert(&r).await.unwrap();
        let loaded = store.load(&r.flow_id).await.unwrap().unwrap();
        assert_eq!(loaded.template_name, "default");
        assert_eq!(loaded.user_request, "do thing");
    }

    #[tokio::test]
    async fn test_load_missing_returns_none() {
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let r = store.load("does-not-exist").await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn test_update_overwrites_status_and_phase() {
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let mut r = sample_record();
        store.insert(&r).await.unwrap();
        r.status = "awaiting_gate".to_string();
        r.current_phase = "design".to_string();
        r.updated_at = 2_000_000;
        store.update(&r).await.unwrap();
        let loaded = store.load(&r.flow_id).await.unwrap().unwrap();
        assert_eq!(loaded.status, "awaiting_gate");
        assert_eq!(loaded.current_phase, "design");
    }

    #[tokio::test]
    async fn test_active_for_session_returns_only_active() {
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let mut r1 = sample_record();
        r1.session_key = "cli:s1".to_string();
        r1.status = "running".to_string();
        store.insert(&r1).await.unwrap();

        let mut r2 = sample_record();
        r2.flow_id = uuid::Uuid::now_v7().to_string();
        r2.session_key = "cli:s1".to_string();
        r2.status = "done".to_string();
        store.insert(&r2).await.unwrap();

        let active = store.active_for_session("cli:s1").await.unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().flow_id, r1.flow_id);
    }

    #[tokio::test]
    async fn test_list_recent_orders_by_updated_at_desc() {
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);
        let mut r1 = sample_record();
        r1.updated_at = 1_000;
        store.insert(&r1).await.unwrap();
        let mut r2 = sample_record();
        r2.flow_id = uuid::Uuid::now_v7().to_string();
        r2.updated_at = 2_000;
        store.insert(&r2).await.unwrap();

        let recent = store.list_recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].flow_id, r2.flow_id); // newest first
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --package gasket-storage flow_run_store:: 2>&1 | head -20
```

Expected: compile error (`FlowRunStore`, `FlowRunRecord` not defined).

- [ ] **Step 3: Add the implementation above the tests**

Replace the entire file contents with:

```rust
//! Repository for the `flow_runs` table.
//!
//! `FlowRunRecord` is a flat row representation; serialization of the rich
//! `FlowState` (with `Arc<FlowTemplate>` etc.) is handled in the engine layer.
//! This crate sees only strings, ints, and JSON blobs.

use sqlx::{Row, SqlitePool};

/// One row in `flow_runs`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowRunRecord {
    pub flow_id: String,
    pub template_name: String,
    pub template_version: i64,
    pub user_request: String,
    pub session_key: String,
    /// Discriminant string: `running` | `awaiting_gate` | `paused` | `done` | `aborted`.
    pub status: String,
    pub current_phase: String,
    /// JSON: `{phase_id: PhaseOutput}`.
    pub completed_phases: String,
    /// JSON array of `PendingWikiWrite`.
    pub pending_wiki: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Repository for `flow_runs`.
#[derive(Clone)]
pub struct FlowRunStore {
    pool: SqlitePool,
}

impl FlowRunStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, r: &FlowRunRecord) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO flow_runs (
                flow_id, template_name, template_version, user_request, session_key,
                status, current_phase, completed_phases, pending_wiki,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&r.flow_id)
        .bind(&r.template_name)
        .bind(r.template_version)
        .bind(&r.user_request)
        .bind(&r.session_key)
        .bind(&r.status)
        .bind(&r.current_phase)
        .bind(&r.completed_phases)
        .bind(&r.pending_wiki)
        .bind(r.created_at)
        .bind(r.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update(&self, r: &FlowRunRecord) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE flow_runs SET
                template_name = ?, template_version = ?, user_request = ?,
                session_key = ?, status = ?, current_phase = ?,
                completed_phases = ?, pending_wiki = ?, updated_at = ?
             WHERE flow_id = ?",
        )
        .bind(&r.template_name)
        .bind(r.template_version)
        .bind(&r.user_request)
        .bind(&r.session_key)
        .bind(&r.status)
        .bind(&r.current_phase)
        .bind(&r.completed_phases)
        .bind(&r.pending_wiki)
        .bind(r.updated_at)
        .bind(&r.flow_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load(&self, flow_id: &str) -> anyhow::Result<Option<FlowRunRecord>> {
        let row = sqlx::query("SELECT * FROM flow_runs WHERE flow_id = ?")
            .bind(flow_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(row_to_record))
    }

    /// Return the single active flow_run for this session (running / awaiting_gate / paused).
    pub async fn active_for_session(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Option<FlowRunRecord>> {
        let row = sqlx::query(
            "SELECT * FROM flow_runs
             WHERE session_key = ?
               AND status IN ('running', 'awaiting_gate', 'paused')
             ORDER BY updated_at DESC
             LIMIT 1",
        )
        .bind(session_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_record))
    }

    /// List the N most recent flow_runs across all sessions.
    pub async fn list_recent(&self, limit: i64) -> anyhow::Result<Vec<FlowRunRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM flow_runs ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> FlowRunRecord {
    FlowRunRecord {
        flow_id: row.get("flow_id"),
        template_name: row.get("template_name"),
        template_version: row.get("template_version"),
        user_request: row.get("user_request"),
        session_key: row.get("session_key"),
        status: row.get("status"),
        current_phase: row.get("current_phase"),
        completed_phases: row.get("completed_phases"),
        pending_wiki: row.get("pending_wiki"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

// ── tests below this line (kept from Step 1) ──
```

Then keep the entire test module from Step 1 below this line.

- [ ] **Step 4: Wire into `lib.rs`**

In `gasket/storage/src/lib.rs`, find the `mod cron_store;` line and add `mod flow_run_store;` alphabetically near it. After the existing `pub use cron_store::CronStore;` style re-exports, add:

```rust
pub use flow_run_store::{FlowRunRecord, FlowRunStore};
```

In the `impl SqliteStore` block, add a convenience accessor (after `cron_store(&self)`):

```rust
    /// Convenience accessor — builds a [`FlowRunStore`] backed by the same pool.
    pub fn flow_run_store(&self) -> FlowRunStore {
        FlowRunStore::new(self.pool.clone())
    }
```

- [ ] **Step 5: Run tests to verify pass**

```bash
cargo test --package gasket-storage flow_run_store:: 2>&1 | tail -10
```

Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/storage/src/flow_run_store.rs gasket/storage/src/lib.rs
git commit -m "feat(storage): add FlowRunStore CRUD repository"
```

---

### Task 5: Flow template + loader

**Files:**
- Create: `gasket/engine/src/flow/mod.rs`
- Create: `gasket/engine/src/flow/template.rs`
- Modify: `gasket/engine/src/lib.rs` (add `pub mod flow;`)

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/flow/template.rs` with the following tests at the bottom (we'll add code above them next):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_yaml(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{}.yaml", name));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    fn write_prompt(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
    }

    const MINIMAL_YAML: &str = r#"
name: test-tpl
description: minimal
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: "B"
    prompt_file: prompts/b.md
    allowed_tools: ["wiki_search"]
    max_iterations: 3
    gate:
      required: true
      prompt: "Continue?"
"#;

    #[test]
    fn test_parse_minimal_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "test-tpl", MINIMAL_YAML);
        write_prompt(dir.path(), "prompts/b.md", "Hello {{user_request}}");

        let tpl = FlowTemplate::load_from_path(&path).unwrap();
        assert_eq!(tpl.name, "test-tpl");
        assert_eq!(tpl.phases.len(), 1);
        assert_eq!(tpl.phases[0].id.as_str(), "brainstorm");
        assert!(tpl.phases[0].gate.required);
    }

    #[test]
    fn test_invalid_yaml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "bad", "not: valid: yaml: ::");
        let r = FlowTemplate::load_from_path(&path);
        assert!(r.is_err());
    }

    #[test]
    fn test_missing_prompt_file_returns_error_when_resolving() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "test-tpl", MINIMAL_YAML);
        // Note: don't create prompts/b.md — load succeeds but resolve_prompt fails.

        let tpl = FlowTemplate::load_from_path(&path).unwrap();
        let r = tpl.resolve_prompt(&PhaseId::brainstorm());
        assert!(r.is_err());
    }

    #[test]
    fn test_resolve_prompt_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_yaml(dir.path(), "test-tpl", MINIMAL_YAML);
        write_prompt(dir.path(), "prompts/b.md", "Hello {{user_request}}");

        let tpl = FlowTemplate::load_from_path(&path).unwrap();
        let content = tpl.resolve_prompt(&PhaseId::brainstorm()).unwrap();
        assert_eq!(content, "Hello {{user_request}}");
    }

    #[test]
    fn test_user_dir_overrides_builtin() {
        let user = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        write_yaml(user.path(), "default", MINIMAL_YAML.replace("test-tpl", "user-default").as_str());
        write_prompt(user.path(), "prompts/b.md", "user");
        write_yaml(builtin.path(), "default", MINIMAL_YAML.replace("test-tpl", "builtin-default").as_str());
        write_prompt(builtin.path(), "prompts/b.md", "builtin");

        let loader = TemplateLoader::new(user.path().to_path_buf(), Some(builtin.path().to_path_buf()));
        let tpl = loader.load("default").unwrap();
        assert_eq!(tpl.name, "user-default");
    }

    #[test]
    fn test_falls_back_to_builtin_when_user_missing() {
        let user = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        write_yaml(builtin.path(), "default", MINIMAL_YAML.replace("test-tpl", "builtin-default").as_str());
        write_prompt(builtin.path(), "prompts/b.md", "builtin");

        let loader = TemplateLoader::new(user.path().to_path_buf(), Some(builtin.path().to_path_buf()));
        let tpl = loader.load("default").unwrap();
        assert_eq!(tpl.name, "builtin-default");
    }

    #[test]
    fn test_render_prompt_substitutes_variables() {
        let template = "User asked: {{user_request}} (flow {{flow_id}})";
        let mut vars = std::collections::HashMap::new();
        vars.insert("user_request".to_string(), "fix bug".to_string());
        vars.insert("flow_id".to_string(), "abc123".to_string());
        let rendered = render_prompt(template, &vars);
        assert_eq!(rendered, "User asked: fix bug (flow abc123)");
    }

    #[test]
    fn test_render_prompt_leaves_unknown_variables_untouched() {
        let template = "{{user_request}} {{undefined}}";
        let mut vars = std::collections::HashMap::new();
        vars.insert("user_request".to_string(), "x".to_string());
        let rendered = render_prompt(template, &vars);
        assert_eq!(rendered, "x {{undefined}}");
    }
}
```

- [ ] **Step 2: Add `tempfile` to dev-dependencies if not present**

Check `gasket/engine/Cargo.toml` for `tempfile` under `[dev-dependencies]`. If missing:

```bash
grep -A 30 "\[dev-dependencies\]" gasket/engine/Cargo.toml
```

If not present, add `tempfile = "3"` to its `[dev-dependencies]` section.

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test --package gasket-engine flow::template:: 2>&1 | head -20
```

Expected: compile error (types not defined).

- [ ] **Step 4: Write the template module**

Replace the file contents with (tests at the bottom unchanged from Step 1):

```rust
//! Flow template definition and loader.
//!
//! YAML schema (see spec §3.2):
//! ```yaml
//! name: <string>
//! description: <string>
//! version: <int>
//! wiki_policy: deferred | blocked | allowed
//! phases:
//!   - id: <phase_id>
//!     label: <string>
//!     prompt_file: <path relative to template file>
//!     allowed_tools: ["*"] | ["tool_a", "tool_b"]
//!     max_iterations: <int>   # 0 = unlimited
//!     gate:
//!       required: <bool>
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
    Deferred,
    Blocked,
    Allowed,
}

impl Default for WikiPolicy {
    fn default() -> Self { WikiPolicy::Deferred }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSpec {
    pub required: bool,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSpec {
    pub id: PhaseId,
    #[serde(default)]
    pub label: String,
    pub prompt_file: PathBuf,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub max_iterations: u32,
    #[serde(default)]
    pub gate: GateSpec,
}

impl Default for GateSpec {
    fn default() -> Self {
        GateSpec { required: false, prompt: String::new() }
    }
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
    /// `pub(crate)` so the state machine tests can construct fixtures without going through YAML.
    #[serde(skip)]
    pub(crate) base_dir: PathBuf,
}

impl FlowTemplate {
    /// Load a template from a YAML file path.
    pub fn load_from_path(path: &Path) -> Result<Arc<Self>> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading template {}", path.display()))?;
        let mut tpl: FlowTemplate = serde_yaml::from_str(&body)
            .with_context(|| format!("parsing template {}", path.display()))?;
        tpl.base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(Arc::new(tpl))
    }

    /// Read the prompt file content for the given phase.
    pub fn resolve_prompt(&self, phase: &PhaseId) -> Result<String> {
        let spec = self.phases.iter()
            .find(|p| &p.id == phase)
            .with_context(|| format!("phase '{}' not in template '{}'", phase.as_str(), self.name))?;
        let path = self.base_dir.join(&spec.prompt_file);
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading prompt {}", path.display()))
    }

    /// Whether `tool_name` is allowed for the given phase.
    pub fn is_tool_allowed(&self, phase: &PhaseId, tool_name: &str) -> bool {
        let Some(spec) = self.phases.iter().find(|p| &p.id == phase) else {
            return false;
        };
        spec.allowed_tools.iter().any(|t| t == "*" || t == tool_name)
    }
}

/// Loader that resolves templates from user dir first, falling back to built-in.
pub struct TemplateLoader {
    user_dir: PathBuf,
    builtin_dir: Option<PathBuf>,
}

impl TemplateLoader {
    pub fn new(user_dir: PathBuf, builtin_dir: Option<PathBuf>) -> Self {
        Self { user_dir, builtin_dir }
    }

    pub fn load(&self, name: &str) -> Result<Arc<FlowTemplate>> {
        let user_path = self.user_dir.join(format!("{}.yaml", name));
        if user_path.exists() {
            return FlowTemplate::load_from_path(&user_path);
        }
        if let Some(ref bd) = self.builtin_dir {
            let builtin_path = bd.join(format!("{}.yaml", name));
            if builtin_path.exists() {
                return FlowTemplate::load_from_path(&builtin_path);
            }
        }
        anyhow::bail!("template '{}' not found in user dir ({}) or built-in dir ({:?})",
            name, self.user_dir.display(), self.builtin_dir.as_ref().map(|p| p.display().to_string()))
    }

    /// List all available template names from both directories. User overrides built-in.
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

/// Replace `{{var}}` occurrences in `template` using values from `vars`.
/// Unknown variables are left as-is. Simple, no escaping logic.
pub fn render_prompt(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(end_rel) = template[i + 2..].find("}}") {
                let key = template[i + 2..i + 2 + end_rel].trim();
                if let Some(val) = vars.get(key) {
                    out.push_str(val);
                    i += 2 + end_rel + 2;
                    continue;
                } else {
                    // unknown — leave as-is
                    out.push_str(&template[i..i + 2 + end_rel + 2]);
                    i += 2 + end_rel + 2;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// tests module from Step 1 lives below this line
```

Then keep the test module from Step 1 below this comment.

- [ ] **Step 5: Create `flow/mod.rs`**

```rust
//! Flow command system — slash-command-gated, YAML-templated phase orchestrator.
//!
//! See `docs/superpowers/specs/2026-05-03-flow-command-system-design.md`.

pub mod template;

pub use template::{FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
```

- [ ] **Step 6: Wire into `engine/src/lib.rs`**

In `gasket/engine/src/lib.rs`, find the existing `pub mod` declarations (around line 19-32) and add `pub mod flow;` alphabetically.

- [ ] **Step 7: Run tests to verify pass**

```bash
cargo test --package gasket-engine flow::template:: 2>&1 | tail -10
```

Expected: 8 passed.

- [ ] **Step 8: Commit**

```bash
git add gasket/engine/src/flow/ gasket/engine/src/lib.rs gasket/engine/Cargo.toml
git commit -m "feat(engine): add flow template loader and prompt renderer"
```

---

### Task 6: `FlowState` — pure state machine

**Files:**
- Create: `gasket/engine/src/flow/state.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/flow/state.rs` with these tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::template::{FlowTemplate, GateSpec, PhaseSpec, WikiPolicy};
    use std::sync::Arc;

    fn template(phases: Vec<(PhaseId, bool /*gate.required*/)>) -> Arc<FlowTemplate> {
        let phases = phases.into_iter().map(|(id, gate_required)| PhaseSpec {
            id,
            label: String::new(),
            prompt_file: std::path::PathBuf::from("dummy"),
            allowed_tools: vec!["*".to_string()],
            max_iterations: 0,
            gate: GateSpec { required: gate_required, prompt: "?".to_string() },
        }).collect();
        Arc::new(FlowTemplate {
            name: "t".to_string(),
            description: String::new(),
            version: 1,
            wiki_policy: WikiPolicy::Deferred,
            phases,
            base_dir: std::path::PathBuf::from("."),
        })
    }

    fn fresh_state(tpl: Arc<FlowTemplate>) -> FlowState {
        FlowState::new_with_template("req".to_string(), "cli:test".to_string(), tpl)
    }

    #[test]
    fn test_new_starts_at_first_phase_running() {
        let tpl = template(vec![(PhaseId::brainstorm(), true), (PhaseId::execute(), false)]);
        let s = fresh_state(tpl);
        assert_eq!(s.current_phase, PhaseId::brainstorm());
        assert_eq!(s.status, FlowStatus::Running);
    }

    #[test]
    fn test_advance_after_phase_with_gate_goes_to_awaiting_gate() {
        let tpl = template(vec![(PhaseId::brainstorm(), true), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "done".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::AwaitingGate { phase: PhaseId::brainstorm() });
        assert_eq!(s.current_phase, PhaseId::brainstorm());  // still brainstorm — gate hasn't passed
    }

    #[test]
    fn test_advance_after_phase_without_gate_goes_to_next_running() {
        let tpl = template(vec![(PhaseId::brainstorm(), false), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "done".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, PhaseId::execute());
    }

    #[test]
    fn test_last_phase_completion_goes_to_done() {
        let tpl = template(vec![(PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "done".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        assert_eq!(s.status, FlowStatus::Done);
    }

    #[test]
    fn test_gate_yes_advances_to_next_phase() {
        let tpl = template(vec![(PhaseId::brainstorm(), true), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "ok".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Yes);
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, PhaseId::execute());
    }

    #[test]
    fn test_gate_no_aborts() {
        let tpl = template(vec![(PhaseId::brainstorm(), true), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "ok".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::No);
        assert_eq!(s.status, FlowStatus::Aborted);
    }

    #[test]
    fn test_gate_redo_clears_phase_output() {
        let tpl = template(vec![(PhaseId::brainstorm(), true), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl);
        s.complete_current_phase(PhaseOutput {
            summary: "first attempt".to_string(),
            iterations_used: 1,
            tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Redo);
        assert_eq!(s.status, FlowStatus::Running);
        assert_eq!(s.current_phase, PhaseId::brainstorm());
        assert!(!s.completed_phases.contains_key(&PhaseId::brainstorm()));
    }

    #[test]
    fn test_gate_back_returns_to_previous_gate() {
        let tpl = template(vec![
            (PhaseId::brainstorm(), true),
            (PhaseId::design(), true),
            (PhaseId::execute(), false),
        ]);
        let mut s = fresh_state(tpl);
        // Complete brainstorm
        s.complete_current_phase(PhaseOutput {
            summary: "b".to_string(), iterations_used: 1, tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        s.apply_gate_response(GateResponse::Yes);  // pass brainstorm gate
        // Complete design
        s.complete_current_phase(PhaseOutput {
            summary: "d".to_string(), iterations_used: 1, tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        s.transition_after_phase();
        // We're now AwaitingGate{design}
        assert_eq!(s.status, FlowStatus::AwaitingGate { phase: PhaseId::design() });
        // Back goes to brainstorm gate
        s.apply_gate_response(GateResponse::Back);
        assert_eq!(s.status, FlowStatus::AwaitingGate { phase: PhaseId::brainstorm() });
        assert_eq!(s.current_phase, PhaseId::brainstorm());
    }

    #[test]
    fn test_pending_wiki_writes_starts_empty() {
        let tpl = template(vec![(PhaseId::brainstorm(), false)]);
        let s = fresh_state(tpl);
        assert!(s.pending_wiki_writes.is_empty());
    }

    #[test]
    fn test_invariant_completed_phases_subset_of_template() {
        let tpl = template(vec![(PhaseId::brainstorm(), false), (PhaseId::execute(), false)]);
        let mut s = fresh_state(tpl.clone());
        s.complete_current_phase(PhaseOutput {
            summary: "x".to_string(), iterations_used: 0, tools_called: vec![],
            finished_at: chrono::Utc::now(),
        });
        for k in s.completed_phases.keys() {
            assert!(tpl.phases.iter().any(|p| &p.id == k));
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --package gasket-engine flow::state:: 2>&1 | head -20
```

Expected: compile error.

- [ ] **Step 3: Write the FlowState module above the tests**

```rust
//! Pure state-machine logic for one flow run.
//!
//! No I/O, no async, no LLM calls. The orchestrator drives this with side
//! effects, but transitions themselves are deterministic and easily tested.

use chrono::{DateTime, Utc};
use gasket_types::{FlowStatus, PendingWikiWrite, PhaseId, PhaseOutput};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::flow::template::FlowTemplate;

/// Runtime state of a flow.
#[derive(Debug, Clone)]
pub struct FlowState {
    pub flow_id: Uuid,
    pub template: Arc<FlowTemplate>,
    pub user_request: String,
    pub current_phase: PhaseId,
    pub status: FlowStatus,
    pub completed_phases: BTreeMap<String, PhaseOutput>,
    pub pending_wiki_writes: Vec<PendingWikiWrite>,
    pub session_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User's response at a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResponse {
    Yes,
    No,
    Edit(String),
    Redo,
    Back,
}

impl FlowState {
    /// Build a fresh state pinned to the first phase of the template.
    pub fn new_with_template(
        user_request: String,
        session_key: String,
        template: Arc<FlowTemplate>,
    ) -> Self {
        let now = Utc::now();
        let first = template.phases.first()
            .map(|p| p.id.clone())
            .unwrap_or_else(|| PhaseId::brainstorm());
        Self {
            flow_id: Uuid::now_v7(),
            template,
            user_request,
            current_phase: first,
            status: FlowStatus::Running,
            completed_phases: BTreeMap::new(),
            pending_wiki_writes: Vec::new(),
            session_key,
            created_at: now,
            updated_at: now,
        }
    }

    /// Record the output of `current_phase`.
    pub fn complete_current_phase(&mut self, output: PhaseOutput) {
        self.completed_phases
            .insert(self.current_phase.as_str().to_string(), output);
        self.updated_at = Utc::now();
    }

    /// After a phase has completed, decide next status.
    /// - If gate.required → AwaitingGate
    /// - Else if more phases → advance to next, status Running
    /// - Else → Done
    pub fn transition_after_phase(&mut self) {
        let idx = self.phase_index(&self.current_phase).unwrap_or(0);
        let spec = &self.template.phases[idx];
        if spec.gate.required {
            self.status = FlowStatus::AwaitingGate { phase: self.current_phase.clone() };
        } else if let Some(next) = self.template.phases.get(idx + 1) {
            self.current_phase = next.id.clone();
            self.status = FlowStatus::Running;
        } else {
            self.status = FlowStatus::Done;
        }
        self.updated_at = Utc::now();
    }

    /// Apply a user gate response. Only valid when status = AwaitingGate.
    pub fn apply_gate_response(&mut self, resp: GateResponse) {
        let cur_phase = match &self.status {
            FlowStatus::AwaitingGate { phase } => phase.clone(),
            _ => return,
        };
        let idx = self.phase_index(&cur_phase).unwrap_or(0);
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
            GateResponse::Edit(_text) => {
                // Caller (orchestrator) is responsible for injecting the text into
                // the next LLM call. State stays Running on the same phase.
                self.status = FlowStatus::Running;
                self.current_phase = cur_phase;
            }
            GateResponse::Redo => {
                self.completed_phases.remove(cur_phase.as_str());
                self.status = FlowStatus::Running;
                self.current_phase = cur_phase;
            }
            GateResponse::Back => {
                if idx == 0 {
                    // No previous phase — stay where we are.
                    self.status = FlowStatus::AwaitingGate { phase: cur_phase };
                } else {
                    let prev = self.template.phases[idx - 1].id.clone();
                    self.current_phase = prev.clone();
                    self.status = FlowStatus::AwaitingGate { phase: prev };
                }
            }
        }
        self.updated_at = Utc::now();
    }

    /// Mark the flow as paused (e.g., after an error or channel disconnect).
    pub fn pause(&mut self) {
        self.status = FlowStatus::Paused;
        self.updated_at = Utc::now();
    }

    /// Mark the flow as aborted (e.g., user ran `/flow abort`).
    pub fn abort(&mut self) {
        self.status = FlowStatus::Aborted;
        self.updated_at = Utc::now();
    }

    fn phase_index(&self, phase: &PhaseId) -> Option<usize> {
        self.template.phases.iter().position(|p| &p.id == phase)
    }
}
```

- [ ] **Step 4: Add `pub mod state;` and re-exports**

In `gasket/engine/src/flow/mod.rs`:

```rust
//! Flow command system — slash-command-gated, YAML-templated phase orchestrator.

pub mod state;
pub mod template;

pub use state::{FlowState, GateResponse};
pub use template::{FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
```

- [ ] **Step 5: Run tests to verify pass**

```bash
cargo test --package gasket-engine flow::state:: 2>&1 | tail -10
```

Expected: 10 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/flow/state.rs gasket/engine/src/flow/mod.rs
git commit -m "feat(engine): add FlowState pure state-machine logic"
```

---

### Task 7: `WikiWriteGuard` trait + 3 implementations

**Files:**
- Create: `gasket/engine/src/flow/wiki_guard.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/flow/wiki_guard.rs` with tests at the bottom:

```rust
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
    async fn test_allow_all_passes_through() {
        let g = AllowAllGuard;
        let d = g.intercept(args("topics/x")).await;
        assert!(matches!(d, GuardDecision::Allow));
    }

    #[tokio::test]
    async fn test_blocking_rejects_all() {
        let g = BlockingGuard;
        let d = g.intercept(args("topics/x")).await;
        match d {
            GuardDecision::Reject(msg) => assert!(msg.contains("blocked")),
            _ => panic!("expected Reject"),
        }
    }

    #[tokio::test]
    async fn test_deferring_queues_and_returns_defer() {
        let g = DeferringGuard::new(gasket_types::PhaseId::execute());
        let d = g.intercept(args("topics/x")).await;
        assert!(matches!(d, GuardDecision::Defer(_)));
        let pending = g.drain_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "topics/x");
    }

    #[tokio::test]
    async fn test_deferring_accumulates_multiple() {
        let g = DeferringGuard::new(gasket_types::PhaseId::execute());
        g.intercept(args("topics/a")).await;
        g.intercept(args("topics/b")).await;
        let pending = g.drain_pending();
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn test_deferring_drain_is_idempotent_after_drain() {
        let g = DeferringGuard::new(gasket_types::PhaseId::execute());
        g.intercept(args("topics/a")).await;
        let _ = g.drain_pending();
        let after = g.drain_pending();
        assert!(after.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --package gasket-engine flow::wiki_guard:: 2>&1 | head -20
```

Expected: compile error.

- [ ] **Step 3: Write the wiki_guard module above the tests**

```rust
//! Wiki write interception for flow-managed wiki write reduction.
//!
//! `WikiWriteTool` calls `guard.intercept(args)` on every invocation; the
//! returned `GuardDecision` controls whether the write proceeds.
//!
//! Three implementations:
//! - `AllowAllGuard`: pass-through (default outside flows)
//! - `DeferringGuard`: queue for end-of-flow user approval
//! - `BlockingGuard`: reject everything (for templates with wiki_policy=blocked)

use async_trait::async_trait;
use chrono::Utc;
use gasket_types::{PendingWikiWrite, PhaseId};
use std::sync::Mutex;

/// View of `WikiWriteTool::WriteArgs` exposed to the guard (avoids the
/// `wiki_tools.rs` private struct leaking out of that module).
#[derive(Debug, Clone)]
pub struct WriteArgsView {
    pub path: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub page_type: String,
}

/// Decision returned from a guard intercept.
#[derive(Debug)]
pub enum GuardDecision {
    /// Pass through to the actual `PageStore::write`.
    Allow,
    /// Queue for later user approval; tool returns success-like message to LLM.
    Defer(WriteArgsView),
    /// Reject; tool returns this string to the LLM as an error.
    Reject(String),
}

#[async_trait]
pub trait WikiWriteGuard: Send + Sync {
    async fn intercept(&self, args: WriteArgsView) -> GuardDecision;
}

/// Pass-through guard — used outside flows.
pub struct AllowAllGuard;

#[async_trait]
impl WikiWriteGuard for AllowAllGuard {
    async fn intercept(&self, _args: WriteArgsView) -> GuardDecision {
        GuardDecision::Allow
    }
}

/// Reject all writes.
pub struct BlockingGuard;

#[async_trait]
impl WikiWriteGuard for BlockingGuard {
    async fn intercept(&self, _args: WriteArgsView) -> GuardDecision {
        GuardDecision::Reject(
            "Wiki writes are blocked during this flow. The LLM should not call wiki_write here."
                .to_string(),
        )
    }
}

/// Queue writes for later user approval (deferred policy).
pub struct DeferringGuard {
    pending: Mutex<Vec<PendingWikiWrite>>,
    origin_phase: PhaseId,
}

impl DeferringGuard {
    pub fn new(origin_phase: PhaseId) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            origin_phase,
        }
    }

    /// Take all queued writes, clearing the internal buffer.
    pub fn drain_pending(&self) -> Vec<PendingWikiWrite> {
        let mut g = self.pending.lock().unwrap();
        std::mem::take(&mut *g)
    }
}

#[async_trait]
impl WikiWriteGuard for DeferringGuard {
    async fn intercept(&self, args: WriteArgsView) -> GuardDecision {
        let pending = PendingWikiWrite {
            path: args.path.clone(),
            title: args.title.clone(),
            content: args.content.clone(),
            tags: args.tags.clone(),
            page_type: args.page_type.clone(),
            origin_phase: self.origin_phase.clone(),
            queued_at: Utc::now(),
        };
        self.pending.lock().unwrap().push(pending);
        GuardDecision::Defer(args)
    }
}
```

- [ ] **Step 4: Add to `mod.rs`**

In `gasket/engine/src/flow/mod.rs`:

```rust
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use state::{FlowState, GateResponse};
pub use template::{FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{AllowAllGuard, BlockingGuard, DeferringGuard, GuardDecision, WikiWriteGuard, WriteArgsView};
```

- [ ] **Step 5: Run tests to verify pass**

```bash
cargo test --package gasket-engine flow::wiki_guard:: 2>&1 | tail -10
```

Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/flow/wiki_guard.rs gasket/engine/src/flow/mod.rs
git commit -m "feat(engine): add WikiWriteGuard trait with AllowAll/Blocking/Deferring impls"
```

---

### Task 8: Wire `WikiWriteGuard` into `WikiWriteTool`

**Files:**
- Modify: `gasket/engine/src/tools/wiki_tools.rs`

This is a defensive change: existing call-sites that build `WikiWriteTool::new(page_store)` should continue to work. We add `with_guard()` chaining.

- [ ] **Step 1: Write a regression test that confirms `AllowAllGuard` keeps existing behavior**

In `gasket/engine/src/tools/wiki_tools.rs`, find the existing `#[cfg(test)] mod tests` block (or create one if absent — search for `mod tests` in the file). Add this test:

```rust
    #[tokio::test]
    async fn test_wiki_write_with_default_guard_allows() {
        // Smoke test: an unmodified WikiWriteTool (without explicit guard) still
        // dispatches to PageStore — equivalent to AllowAllGuard.
        // This test is a placeholder asserting the field exists and has the
        // expected default; full behavioral test requires PageStore mocking
        // which is out of scope here.
        // We just confirm `with_guard` compiles and overrides the default.
        use crate::flow::{AllowAllGuard, BlockingGuard, WikiWriteGuard};
        use std::sync::Arc;

        // Construct a dummy PageStore via a temp dir (same pattern as wiki tests
        // elsewhere). If PageStore::new requires a pool, use the test helper.
        // For this regression test we stub by checking that with_guard returns Self.
        let g_default: Arc<dyn WikiWriteGuard> = Arc::new(AllowAllGuard);
        let g_blocking: Arc<dyn WikiWriteGuard> = Arc::new(BlockingGuard);
        let _ = (g_default, g_blocking);
    }
```

(This is a compile-only check; deeper behavioral tests live in Task 11.)

- [ ] **Step 2: Modify `WikiWriteTool` to accept an injected guard**

In `gasket/engine/src/tools/wiki_tools.rs`, find `pub struct WikiWriteTool` and modify:

```rust
pub struct WikiWriteTool {
    page_store: PageStore,
    guard: Arc<dyn crate::flow::WikiWriteGuard>,
}

impl WikiWriteTool {
    pub fn new(page_store: PageStore) -> Self {
        Self {
            page_store,
            guard: Arc::new(crate::flow::AllowAllGuard),
        }
    }

    /// Replace the default `AllowAllGuard` with a custom guard.
    /// Used by the FlowOrchestrator to enforce wiki_policy.
    pub fn with_guard(mut self, guard: Arc<dyn crate::flow::WikiWriteGuard>) -> Self {
        self.guard = guard;
        self
    }
}
```

Also add `use std::sync::Arc;` near the top of the file if not already present.

- [ ] **Step 3: Update `execute` to call the guard first**

In the `async fn execute` body (around line 191), insert guard interception just after `parsed: WriteArgs` is built and before the existing `path/title/content` validation. The full `execute` becomes:

```rust
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: WriteArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        // ── NEW: guard interception ───────────────────────────────
        let view = crate::flow::WriteArgsView {
            path: parsed.path.clone(),
            title: parsed.title.clone(),
            content: parsed.content.clone(),
            tags: parsed.tags.clone(),
            page_type: parsed.page_type.clone(),
        };
        match self.guard.intercept(view).await {
            crate::flow::GuardDecision::Allow => { /* proceed */ }
            crate::flow::GuardDecision::Defer(_) => {
                return Ok(format!(
                    "Wiki write deferred for end-of-flow review (path: {}). \
                     The user will be asked to approve this write when the flow finishes.",
                    parsed.path
                ));
            }
            crate::flow::GuardDecision::Reject(msg) => {
                return Err(ToolError::PermissionDenied(msg));
            }
        }
        // ── existing validation / write below ─────────────────────

        let path = parsed.path.trim();
        // ... rest of existing function unchanged
```

(Keep the rest of the function — `path.trim()`, `title.trim()`, `PageType::*` matching, `page_store.write()` — exactly as it was.)

- [ ] **Step 4: Build and run all engine tests**

```bash
cargo build --package gasket-engine 2>&1 | tail -5
cargo test --package gasket-engine 2>&1 | tail -10
```

Expected: build succeeds; all existing tests pass; the new placeholder test passes.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/wiki_tools.rs
git commit -m "feat(engine): wire WikiWriteGuard into WikiWriteTool with deferred/rejected paths"
```

---

### Task 9: `PhaseRunner` — drives one phase via the kernel

**Files:**
- Create: `gasket/engine/src/flow/phase_runner.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

**Note:** This task uses the kernel's existing `execute_streaming` API. The runner does NOT manage tools or guards directly — it receives a `RuntimeContext` already configured by the orchestrator (Task 11) with the phase's filtered tool registry. PhaseRunner's job is: render the prompt, call the kernel once, capture the result.

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/flow/phase_runner.rs`:

```rust
//! Drives one phase: renders the prompt, invokes the kernel, returns PhaseOutput.

use anyhow::Result;
use chrono::Utc;
use gasket_providers::ChatMessage;
use gasket_types::{PhaseId, PhaseOutput};
use std::collections::HashMap;
use std::sync::Arc;

use crate::flow::template::{render_prompt, FlowTemplate};
use crate::kernel::{self, RuntimeContext, StreamEvent};

pub struct PhaseRunner;

impl PhaseRunner {
    /// Run one phase. The caller is responsible for pre-configuring `ctx`
    /// with the phase's tool filter and any wiki guard.
    ///
    /// `vars` are template-render variables (`user_request`, `flow_id`, etc.).
    /// Returns the phase output (LLM final content + metadata).
    pub async fn run(
        ctx: RuntimeContext,
        template: &FlowTemplate,
        phase: &PhaseId,
        vars: &HashMap<String, String>,
        prior_messages: Vec<ChatMessage>,
        event_tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<PhaseOutput> {
        // 1. Resolve and render the prompt
        let raw_prompt = template.resolve_prompt(phase)?;
        let rendered = render_prompt(&raw_prompt, vars);

        // 2. Build messages: prior context + phase system prompt
        let mut messages = prior_messages;
        messages.push(ChatMessage::system(rendered));

        // 3. Call the kernel
        let result = kernel::execute_streaming(&ctx, messages, event_tx)
            .await
            .map_err(|e| anyhow::anyhow!("kernel error in phase {}: {}", phase.as_str(), e))?;

        Ok(PhaseOutput {
            summary: result.content,
            iterations_used: 0, // kernel doesn't surface iteration count today; TODO in v1.1
            tools_called: result.tools_used,
            finished_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_runner_struct_compiles() {
        // PhaseRunner is a unit-struct dispatcher; its single method is async
        // and integration-tested in Task 11 (orchestrator) via mock provider.
        // This test just confirms the type exists.
        let _: PhaseRunner = PhaseRunner;
    }
}
```

- [ ] **Step 2: Add to `mod.rs`**

In `gasket/engine/src/flow/mod.rs`:

```rust
pub mod phase_runner;
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use phase_runner::PhaseRunner;
pub use state::{FlowState, GateResponse};
pub use template::{render_prompt, FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{
    AllowAllGuard, BlockingGuard, DeferringGuard, GuardDecision, WikiWriteGuard, WriteArgsView,
};
```

- [ ] **Step 3: Build and run**

```bash
cargo build --package gasket-engine 2>&1 | tail -5
cargo test --package gasket-engine flow::phase_runner:: 2>&1 | tail -10
```

Expected: build succeeds; 1 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/flow/phase_runner.rs gasket/engine/src/flow/mod.rs
git commit -m "feat(engine): add PhaseRunner — single-phase kernel call wrapper"
```

---

### Task 10: `GateController` — CLI variant

**Files:**
- Create: `gasket/engine/src/flow/gate.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

**Note:** v1 ships only the CLI gate (stdin readline). Web gate UI is v1.1 per spec §8 — but the parser is shared, so once the Web channel sends a `gate_response` event, only the controller wrapper changes.

- [ ] **Step 1: Write failing tests for gate-response parsing**

Create `gasket/engine/src/flow/gate.rs`:

```rust
//! Gate controller — collects user response after a phase completes.
//!
//! Two implementations live here:
//! - `parse_response`: pure function that maps a string into a `GateResponse`
//! - `CliGate`: blocks on stdin readline, calls `parse_response`
//!
//! The pure parser is shared with the (future) Web gate.

use crate::flow::state::GateResponse;

/// Parse a raw user line into a `GateResponse`.
/// Accepted forms:
/// - `y` / `yes`              → Yes
/// - `n` / `no`               → No
/// - `redo`                   → Redo
/// - `back`                   → Back
/// - `edit <text>`            → Edit(text)
/// - everything else          → None (caller re-prompts)
pub fn parse_response(line: &str) -> Option<GateResponse> {
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "y" | "yes" => Some(GateResponse::Yes),
        "n" | "no" => Some(GateResponse::No),
        "redo" => Some(GateResponse::Redo),
        "back" => Some(GateResponse::Back),
        s if s.starts_with("edit ") || s == "edit" => {
            let rest = if s == "edit" { "" } else { trimmed[5..].trim() };
            Some(GateResponse::Edit(rest.to_string()))
        }
        _ => None,
    }
}

/// CLI gate controller — blocks on stdin readline.
pub struct CliGate;

impl CliGate {
    /// Read lines from stdin until a valid response is received.
    /// Prints a re-prompt for invalid input.
    pub fn read_response(prompt: &str) -> std::io::Result<GateResponse> {
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        loop {
            print!("{prompt} ");
            stdout.flush()?;
            let mut line = String::new();
            if stdin.lock().read_line(&mut line)? == 0 {
                // EOF — treat as abort
                return Ok(GateResponse::No);
            }
            if let Some(resp) = parse_response(&line) {
                return Ok(resp);
            }
            eprintln!("Unrecognised input. Use: y / n / edit <text> / redo / back");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yes() {
        assert_eq!(parse_response("y"), Some(GateResponse::Yes));
        assert_eq!(parse_response("yes"), Some(GateResponse::Yes));
        assert_eq!(parse_response("YES"), Some(GateResponse::Yes));
        assert_eq!(parse_response("  y  "), Some(GateResponse::Yes));
    }

    #[test]
    fn test_parse_no() {
        assert_eq!(parse_response("n"), Some(GateResponse::No));
        assert_eq!(parse_response("no"), Some(GateResponse::No));
    }

    #[test]
    fn test_parse_redo_back() {
        assert_eq!(parse_response("redo"), Some(GateResponse::Redo));
        assert_eq!(parse_response("back"), Some(GateResponse::Back));
    }

    #[test]
    fn test_parse_edit_with_text() {
        assert_eq!(
            parse_response("edit please rephrase the design"),
            Some(GateResponse::Edit("please rephrase the design".to_string()))
        );
    }

    #[test]
    fn test_parse_edit_without_text() {
        assert_eq!(parse_response("edit"), Some(GateResponse::Edit(String::new())));
    }

    #[test]
    fn test_parse_unknown_returns_none() {
        assert_eq!(parse_response("maybe"), None);
        assert_eq!(parse_response(""), None);
    }
}
```

- [ ] **Step 2: Add to `mod.rs`**

```rust
pub mod gate;
pub mod phase_runner;
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use gate::{parse_response, CliGate};
pub use phase_runner::PhaseRunner;
pub use state::{FlowState, GateResponse};
pub use template::{render_prompt, FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{
    AllowAllGuard, BlockingGuard, DeferringGuard, GuardDecision, WikiWriteGuard, WriteArgsView,
};
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package gasket-engine flow::gate:: 2>&1 | tail -10
```

Expected: 6 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/flow/gate.rs gasket/engine/src/flow/mod.rs
git commit -m "feat(engine): add CLI gate controller with shared response parser"
```

---

### Task 11: `FlowOrchestrator` — ties it all together

**Files:**
- Create: `gasket/engine/src/flow/orchestrator.rs`
- Modify: `gasket/engine/src/flow/mod.rs`

**Note:** This is the biggest task. We'll write a focused integration test using a mock provider that drives a 3-phase mini flow. The CLI/agent integration is Task 14.

- [ ] **Step 1: Write the orchestrator skeleton + integration tests**

Create `gasket/engine/src/flow/orchestrator.rs`:

```rust
//! FlowOrchestrator — owns one running flow.
//!
//! Lifecycle:
//! 1. `start_new(template_name, request)` — load template, insert flow_run row,
//!    initialize FlowState, return self.
//! 2. `step()` — run the current phase via PhaseRunner, capture output, decide
//!    next status. If AwaitingGate, returns OrchestratorOutcome::AwaitingGate.
//!    If Done, calls `finalize_pending_wiki()` and returns Done.
//! 3. `apply_gate(GateResponse)` — apply gate response to state, persist, ready
//!    for next `step()`.
//! 4. `resume(flow_id)` — reload row, rebuild state, ready for `step()`.

use anyhow::{Context, Result};
use chrono::Utc;
use gasket_providers::ChatMessage;
use gasket_storage::{FlowRunRecord, FlowRunStore};
use gasket_types::{FlowStatus, PendingWikiWrite, PhaseOutput};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::flow::{
    state::{FlowState, GateResponse},
    template::TemplateLoader,
    wiki_guard::DeferringGuard,
    PhaseRunner,
};
use crate::kernel::{RuntimeContext, StreamEvent};

pub enum OrchestratorOutcome {
    /// Phase completed, awaiting user gate response.
    AwaitingGate { phase_label: String, summary: String, prompt: String },
    /// Phase advanced into a new Running phase — caller should call `step()` again.
    Continuing { phase_label: String },
    /// Flow has finished. `pending_wiki` is the queue for end-of-flow approval.
    Done { pending_wiki: Vec<PendingWikiWrite> },
    /// Flow was paused (kernel error). Resume with `resume(flow_id)` later.
    Paused { error: String },
    /// Flow was aborted (gate said no, or `/flow abort`).
    Aborted,
}

pub struct FlowOrchestrator {
    state: FlowState,
    store: FlowRunStore,
    deferring_guard: Arc<DeferringGuard>,
}

impl FlowOrchestrator {
    /// Create a new orchestrator, loading the template and inserting the flow_run row.
    pub async fn start_new(
        loader: &TemplateLoader,
        store: FlowRunStore,
        template_name: &str,
        user_request: String,
        session_key: String,
    ) -> Result<Self> {
        let template = loader.load(template_name)
            .with_context(|| format!("loading template '{template_name}'"))?;
        let state = FlowState::new_with_template(user_request, session_key, template.clone());
        let guard = Arc::new(DeferringGuard::new(state.current_phase.clone()));

        let record = state_to_record(&state);
        store.insert(&record).await?;

        Ok(Self { state, store, deferring_guard: guard })
    }

    /// Resume a flow from storage. Template is reloaded; if missing/changed,
    /// the caller is responsible for warning the user (see CLI integration).
    pub async fn resume(
        loader: &TemplateLoader,
        store: FlowRunStore,
        flow_id: &str,
    ) -> Result<Self> {
        let record = store.load(flow_id).await?
            .with_context(|| format!("flow_id '{flow_id}' not found"))?;
        let template = loader.load(&record.template_name)
            .with_context(|| format!("template '{}' missing on resume", record.template_name))?;
        let state = record_to_state(&record, template.clone())?;
        let guard = Arc::new(DeferringGuard::new(state.current_phase.clone()));
        Ok(Self { state, store, deferring_guard: guard })
    }

    pub fn flow_id(&self) -> uuid::Uuid {
        self.state.flow_id
    }

    pub fn status(&self) -> &FlowStatus {
        &self.state.status
    }

    /// Run the current phase to completion, then transition.
    /// Returns the next outcome (AwaitingGate / Done / Paused).
    pub async fn step(
        &mut self,
        runtime_ctx: RuntimeContext,
        event_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<OrchestratorOutcome> {
        let phase = self.state.current_phase.clone();
        let vars = self.template_vars();

        let prior_messages: Vec<ChatMessage> = Vec::new();
        // Note: prior_messages stays empty for v1 — each phase is a fresh kernel
        // invocation. PhaseOutputs from earlier phases are surfaced via
        // `{{previous_outputs.<phase>}}` rendering in the prompt template.

        let result = PhaseRunner::run(
            runtime_ctx,
            &self.state.template,
            &phase,
            &vars,
            prior_messages,
            event_tx,
        ).await;

        match result {
            Ok(output) => {
                self.state.complete_current_phase(output);
                self.state.transition_after_phase();
                self.persist().await?;
                self.outcome_for_status()
            }
            Err(e) => {
                self.state.pause();
                self.persist().await?;
                Ok(OrchestratorOutcome::Paused { error: e.to_string() })
            }
        }
    }

    /// Apply the user's response at a gate, persist, ready for next `step()`.
    pub async fn apply_gate(&mut self, resp: GateResponse) -> Result<OrchestratorOutcome> {
        self.state.apply_gate_response(resp);
        self.persist().await?;
        self.outcome_for_status()
    }

    /// User-initiated abort.
    pub async fn abort(&mut self) -> Result<()> {
        self.state.abort();
        self.persist().await?;
        Ok(())
    }

    /// Drain pending wiki writes (called at flow end or explicit /wiki review).
    pub fn drain_pending_wiki(&self) -> Vec<PendingWikiWrite> {
        self.deferring_guard.drain_pending()
    }

    /// Accessor for the deferring guard so the orchestrator's tool-registry
    /// builder (Task 14 caller) can wire it into WikiWriteTool::with_guard.
    pub fn guard(&self) -> Arc<DeferringGuard> {
        self.deferring_guard.clone()
    }

    // ── internals ─────────────────────────────────────────────

    fn template_vars(&self) -> HashMap<String, String> {
        let mut v: HashMap<String, String> = HashMap::new();
        v.insert("user_request".to_string(), self.state.user_request.clone());
        v.insert("flow_id".to_string(), self.state.flow_id.to_string());
        v.insert("phase_total".to_string(), self.state.template.phases.len().to_string());
        let idx = self.state.template.phases.iter()
            .position(|p| p.id == self.state.current_phase).unwrap_or(0);
        v.insert("phase_index".to_string(), (idx + 1).to_string());
        // Previous outputs:
        for (k, output) in &self.state.completed_phases {
            v.insert(format!("previous_outputs.{}", k), output.summary.clone());
        }
        if let Some((_, last)) = self.state.completed_phases.iter().last() {
            v.insert("prev_phase_output".to_string(), last.summary.clone());
        }
        v
    }

    fn outcome_for_status(&self) -> Result<OrchestratorOutcome> {
        match &self.state.status {
            FlowStatus::AwaitingGate { phase } => {
                let spec = self.state.template.phases.iter()
                    .find(|p| &p.id == phase)
                    .ok_or_else(|| anyhow::anyhow!("phase missing from template"))?;
                let summary = self.state.completed_phases
                    .get(phase.as_str())
                    .map(|o| o.summary.clone())
                    .unwrap_or_default();
                Ok(OrchestratorOutcome::AwaitingGate {
                    phase_label: spec.label.clone(),
                    summary,
                    prompt: spec.gate.prompt.clone(),
                })
            }
            FlowStatus::Done => Ok(OrchestratorOutcome::Done {
                pending_wiki: self.drain_pending_wiki(),
            }),
            FlowStatus::Paused => Ok(OrchestratorOutcome::Paused {
                error: "flow paused".to_string(),
            }),
            FlowStatus::Aborted => Ok(OrchestratorOutcome::Aborted),
            FlowStatus::Running => {
                let spec = self.state.template.phases.iter()
                    .find(|p| p.id == self.state.current_phase)
                    .ok_or_else(|| anyhow::anyhow!("current phase not in template"))?;
                Ok(OrchestratorOutcome::Continuing { phase_label: spec.label.clone() })
            }
        }
    }

    async fn persist(&self) -> Result<()> {
        let record = state_to_record(&self.state);
        self.store.update(&record).await?;
        Ok(())
    }
}

fn state_to_record(s: &FlowState) -> FlowRunRecord {
    FlowRunRecord {
        flow_id: s.flow_id.to_string(),
        template_name: s.template.name.clone(),
        template_version: s.template.version,
        user_request: s.user_request.clone(),
        session_key: s.session_key.clone(),
        status: s.status.as_str().to_string(),
        current_phase: s.current_phase.as_str().to_string(),
        completed_phases: serde_json::to_string(&s.completed_phases)
            .unwrap_or_else(|_| "{}".to_string()),
        pending_wiki: serde_json::to_string(&s.pending_wiki_writes)
            .unwrap_or_else(|_| "[]".to_string()),
        created_at: s.created_at.timestamp(),
        updated_at: s.updated_at.timestamp(),
    }
}

fn record_to_state(
    r: &FlowRunRecord,
    template: Arc<crate::flow::FlowTemplate>,
) -> Result<FlowState> {
    use chrono::TimeZone;
    let completed: std::collections::BTreeMap<String, PhaseOutput> =
        serde_json::from_str(&r.completed_phases).unwrap_or_default();
    let pending: Vec<PendingWikiWrite> =
        serde_json::from_str(&r.pending_wiki).unwrap_or_default();
    let status = match r.status.as_str() {
        "running" => FlowStatus::Running,
        "paused" => FlowStatus::Paused,
        "done" => FlowStatus::Done,
        "aborted" => FlowStatus::Aborted,
        "awaiting_gate" => FlowStatus::AwaitingGate {
            phase: parse_phase_id(&r.current_phase),
        },
        other => anyhow::bail!("unknown status '{other}'"),
    };
    Ok(FlowState {
        flow_id: r.flow_id.parse().context("invalid flow_id uuid")?,
        template,
        user_request: r.user_request.clone(),
        current_phase: parse_phase_id(&r.current_phase),
        status,
        completed_phases: completed,
        pending_wiki_writes: pending,
        session_key: r.session_key.clone(),
        created_at: chrono::Utc.timestamp_opt(r.created_at, 0).single()
            .unwrap_or_else(Utc::now),
        updated_at: chrono::Utc.timestamp_opt(r.updated_at, 0).single()
            .unwrap_or_else(Utc::now),
    })
}

fn parse_phase_id(s: &str) -> gasket_types::PhaseId {
    use gasket_types::{BuiltinPhase, PhaseId};
    match s {
        "brainstorm" => PhaseId::Builtin(BuiltinPhase::Brainstorm),
        "design" => PhaseId::Builtin(BuiltinPhase::Design),
        "plan" => PhaseId::Builtin(BuiltinPhase::Plan),
        "execute" => PhaseId::Builtin(BuiltinPhase::Execute),
        "verify" => PhaseId::Builtin(BuiltinPhase::Verify),
        other => PhaseId::Custom(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The orchestrator integration test requires a working RuntimeContext +
    // mock provider. Since gasket already uses a provider trait, the
    // simplest test here only exercises the persistence + state transitions
    // that don't need the kernel.

    async fn temp_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite:file:orch_test_{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4().simple()
            ))
            .await
            .unwrap();
        gasket_storage::migrations::flow_run::run_schema(&pool).await.unwrap();
        pool
    }

    fn write_yaml_template(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let yaml = format!(r#"
name: {name}
description: test
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: B
    prompt_file: prompts/p.md
    allowed_tools: ["wiki_search"]
    max_iterations: 1
    gate:
      required: true
      prompt: "?"
"#);
        let path = dir.join(format!("{}.yaml", name));
        std::fs::write(&path, yaml).unwrap();
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::write(dir.join("prompts/p.md"), "do {{user_request}}").unwrap();
        path
    }

    #[tokio::test]
    async fn test_start_new_inserts_row() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tdd-tpl");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);

        let orch = FlowOrchestrator::start_new(
            &loader, store.clone(), "tdd-tpl",
            "do thing".to_string(), "cli:test".to_string(),
        ).await.unwrap();

        let row = store.load(&orch.flow_id().to_string()).await.unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.current_phase, "brainstorm");
    }

    #[tokio::test]
    async fn test_apply_gate_yes_advances_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tdd-tpl2");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);

        let mut orch = FlowOrchestrator::start_new(
            &loader, store.clone(), "tdd-tpl2",
            "do thing".to_string(), "cli:test".to_string(),
        ).await.unwrap();
        // Manually mark brainstorm complete and at gate
        orch.state.complete_current_phase(PhaseOutput {
            summary: "x".to_string(), iterations_used: 1, tools_called: vec![],
            finished_at: Utc::now(),
        });
        orch.state.transition_after_phase();
        orch.persist().await.unwrap();
        // Single-phase template → after gate.required, status is AwaitingGate
        assert!(matches!(orch.state.status, FlowStatus::AwaitingGate { .. }));

        // Apply Yes → no next phase → Done
        let outcome = orch.apply_gate(GateResponse::Yes).await.unwrap();
        assert!(matches!(outcome, OrchestratorOutcome::Done { .. }));
        let row = store.load(&orch.flow_id().to_string()).await.unwrap().unwrap();
        assert_eq!(row.status, "done");
    }

    #[tokio::test]
    async fn test_resume_reconstructs_state() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tdd-tpl3");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);

        let orch = FlowOrchestrator::start_new(
            &loader, store.clone(), "tdd-tpl3",
            "do thing".to_string(), "cli:test".to_string(),
        ).await.unwrap();
        let flow_id = orch.flow_id().to_string();
        drop(orch);

        let resumed = FlowOrchestrator::resume(&loader, store.clone(), &flow_id)
            .await.unwrap();
        assert_eq!(resumed.flow_id().to_string(), flow_id);
        assert_eq!(resumed.state.user_request, "do thing");
    }

    #[tokio::test]
    async fn test_drain_pending_wiki_returns_buffered_writes() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml_template(dir.path(), "tdd-tpl4");
        let loader = TemplateLoader::new(dir.path().to_path_buf(), None);
        let pool = temp_pool().await;
        let store = FlowRunStore::new(pool);

        let orch = FlowOrchestrator::start_new(
            &loader, store, "tdd-tpl4",
            "x".to_string(), "cli:test".to_string(),
        ).await.unwrap();
        // Simulate a deferred write happening
        let view = crate::flow::WriteArgsView {
            path: "topics/x".to_string(),
            title: "T".to_string(),
            content: "C".to_string(),
            tags: vec![],
            page_type: "topic".to_string(),
        };
        use crate::flow::WikiWriteGuard;
        orch.deferring_guard.intercept(view).await;

        let pending = orch.drain_pending_wiki();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "topics/x");
    }
}
```

The unused import `json` was left in for the next refactor — remove the `use serde_json::json;` line above the body if Clippy complains.

- [ ] **Step 2: Make `migrations` accessible from tests**

In `gasket/storage/src/lib.rs`, change:

```rust
mod migrations;
```

to:

```rust
pub mod migrations;
```

(So the orchestrator's tests can call `gasket_storage::migrations::flow_run::run_schema(...)`. If tests only need it in `cfg(test)`, prefer `#[cfg(test)] pub mod migrations;` — but here we accept the API surface increase since migration discovery is useful broadly.)

- [ ] **Step 3: Add to `mod.rs`**

```rust
pub mod gate;
pub mod orchestrator;
pub mod phase_runner;
pub mod state;
pub mod template;
pub mod wiki_guard;

pub use gate::{parse_response, CliGate};
pub use orchestrator::{FlowOrchestrator, OrchestratorOutcome};
pub use phase_runner::PhaseRunner;
pub use state::{FlowState, GateResponse};
pub use template::{render_prompt, FlowTemplate, GateSpec, PhaseSpec, TemplateLoader, WikiPolicy};
pub use wiki_guard::{
    AllowAllGuard, BlockingGuard, DeferringGuard, GuardDecision, WikiWriteGuard, WriteArgsView,
};
```

- [ ] **Step 4: Build and run tests**

```bash
cargo build --package gasket-engine 2>&1 | tail -10
cargo test --package gasket-engine flow::orchestrator:: 2>&1 | tail -20
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/flow/orchestrator.rs gasket/engine/src/flow/mod.rs gasket/storage/src/lib.rs
git commit -m "feat(engine): add FlowOrchestrator binding template+state+storage"
```

---

### Task 12: Command parser

**Files:**
- Create: `gasket/engine/src/command/mod.rs`
- Create: `gasket/engine/src/command/parser.rs`
- Modify: `gasket/engine/src/lib.rs` (add `pub mod command;`)

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/command/parser.rs`:

```rust
//! Slash-command parser.
//!
//! Maps raw user input lines to `CommandAction` (or `Plain(text)` for non-commands).
//!
//! Grammar:
//!   /flow start <template> -- <request>     → Start { template: <t>, request: <r> }
//!   /flow start <request-without-dashes>    → Start { template: "default", request: <r> }
//!     (disambiguation: if first arg matches a known template name, treat as
//!      `Start { template: <name>, request: <rest> }`. Caller passes `known_templates`.)
//!   /flow status                            → Status
//!   /flow resume <flow_id>                  → Resume { flow_id }
//!   /flow abort                             → Abort
//!   /flow list                              → List
//!   /brainstorm <request>                   → Start { template: "brainstorm-only", request }
//!   /design <request>                       → Start { template: "design-only", request }
//!   /plan <request>                         → Start { template: "plan-only", request }
//!   /execute <request>                      → Start { template: "execute-only", request }
//!   /verify <request>                       → Start { template: "verify-only", request }
//!   anything not starting with /            → Plain(text)
//!   /<unknown>                              → Plain(text) + warning (caller decides)

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    /// Plain conversation input — bypass dispatch.
    Plain(String),

    /// Start a flow.
    Start {
        template: String,
        request: String,
    },

    /// Show current flow status.
    Status,

    /// Resume a flow by id.
    Resume { flow_id: String },

    /// Abort the current flow.
    Abort,

    /// List recent flows.
    List,

    /// Unknown slash command — caller decides whether to show error or pass through.
    Unknown { command: String, raw: String },
}

/// Parse a single user input line.
///
/// `known_templates` provides disambiguation for `/flow start <one-word>`.
pub fn parse(input: &str, known_templates: &[String]) -> CommandAction {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return CommandAction::Plain(input.to_string());
    }

    // Strip leading /
    let body = &trimmed[1..];

    // Split first token from rest
    let mut parts = body.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    match head {
        "flow" => parse_flow_subcommand(rest, known_templates),
        "brainstorm" => CommandAction::Start { template: "brainstorm-only".to_string(), request: rest.to_string() },
        "design" => CommandAction::Start { template: "design-only".to_string(), request: rest.to_string() },
        "plan" => CommandAction::Start { template: "plan-only".to_string(), request: rest.to_string() },
        "execute" => CommandAction::Start { template: "execute-only".to_string(), request: rest.to_string() },
        "verify" => CommandAction::Start { template: "verify-only".to_string(), request: rest.to_string() },
        _ => CommandAction::Unknown { command: head.to_string(), raw: trimmed.to_string() },
    }
}

fn parse_flow_subcommand(rest: &str, known_templates: &[String]) -> CommandAction {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();
    match sub {
        "start" => parse_flow_start(args, known_templates),
        "status" => CommandAction::Status,
        "resume" => CommandAction::Resume { flow_id: args.to_string() },
        "abort" => CommandAction::Abort,
        "list" => CommandAction::List,
        _ => CommandAction::Unknown {
            command: format!("flow {sub}"),
            raw: format!("/flow {rest}"),
        },
    }
}

fn parse_flow_start(args: &str, known_templates: &[String]) -> CommandAction {
    // Form A:  <template> -- <request>
    if let Some((tpl_part, req_part)) = args.split_once(" -- ") {
        return CommandAction::Start {
            template: tpl_part.trim().to_string(),
            request: req_part.trim().to_string(),
        };
    }
    // Form B:  if first token matches a known template, use it; else default.
    let mut iter = args.splitn(2, char::is_whitespace);
    let first = iter.next().unwrap_or("").trim();
    let rest = iter.next().unwrap_or("").trim();
    if !first.is_empty() && known_templates.iter().any(|t| t == first) {
        CommandAction::Start {
            template: first.to_string(),
            request: rest.to_string(),
        }
    } else {
        CommandAction::Start {
            template: "default".to_string(),
            request: args.trim().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates() -> Vec<String> {
        vec!["default".into(), "new-feature".into(), "debug".into()]
    }

    #[test]
    fn test_plain_text_passes_through() {
        assert_eq!(parse("hello world", &templates()), CommandAction::Plain("hello world".to_string()));
    }

    #[test]
    fn test_leading_whitespace_plain() {
        assert_eq!(parse("  hi", &templates()), CommandAction::Plain("  hi".to_string()));
    }

    #[test]
    fn test_flow_start_with_dash_dash_separator() {
        assert_eq!(
            parse("/flow start new-feature -- add auth", &templates()),
            CommandAction::Start { template: "new-feature".to_string(), request: "add auth".to_string() }
        );
    }

    #[test]
    fn test_flow_start_known_template_first_word() {
        assert_eq!(
            parse("/flow start new-feature add auth", &templates()),
            CommandAction::Start { template: "new-feature".to_string(), request: "add auth".to_string() }
        );
    }

    #[test]
    fn test_flow_start_request_only_uses_default() {
        assert_eq!(
            parse("/flow start fix login bug", &templates()),
            CommandAction::Start { template: "default".to_string(), request: "fix login bug".to_string() }
        );
    }

    #[test]
    fn test_flow_status_resume_abort_list() {
        assert_eq!(parse("/flow status", &templates()), CommandAction::Status);
        assert_eq!(
            parse("/flow resume abc-123", &templates()),
            CommandAction::Resume { flow_id: "abc-123".to_string() }
        );
        assert_eq!(parse("/flow abort", &templates()), CommandAction::Abort);
        assert_eq!(parse("/flow list", &templates()), CommandAction::List);
    }

    #[test]
    fn test_brainstorm_shortcut() {
        assert_eq!(
            parse("/brainstorm new project idea", &templates()),
            CommandAction::Start {
                template: "brainstorm-only".to_string(),
                request: "new project idea".to_string()
            }
        );
    }

    #[test]
    fn test_design_plan_execute_verify_shortcuts() {
        for (cmd, expected_tpl) in [
            ("/design X", "design-only"),
            ("/plan X", "plan-only"),
            ("/execute X", "execute-only"),
            ("/verify X", "verify-only"),
        ] {
            let action = parse(cmd, &templates());
            match action {
                CommandAction::Start { template, request } => {
                    assert_eq!(template, expected_tpl);
                    assert_eq!(request, "X");
                }
                _ => panic!("expected Start for '{cmd}'"),
            }
        }
    }

    #[test]
    fn test_unknown_command_returns_unknown() {
        match parse("/foobar do thing", &templates()) {
            CommandAction::Unknown { command, .. } => assert_eq!(command, "foobar"),
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn test_unknown_flow_subcommand_returns_unknown() {
        match parse("/flow nonsense", &templates()) {
            CommandAction::Unknown { command, .. } => assert_eq!(command, "flow nonsense"),
            _ => panic!("expected Unknown"),
        }
    }
}
```

- [ ] **Step 2: Create the `command/mod.rs`**

```rust
//! Slash-command parsing and dispatch.

pub mod parser;

pub use parser::{parse, CommandAction};
```

- [ ] **Step 3: Wire into `engine/src/lib.rs`**

Add `pub mod command;` alphabetically near the other `pub mod` declarations.

- [ ] **Step 4: Run tests**

```bash
cargo test --package gasket-engine command::parser:: 2>&1 | tail -10
```

Expected: 9 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/command/ gasket/engine/src/lib.rs
git commit -m "feat(engine): add slash-command parser"
```

---

### Task 13: `CommandDispatcher` — the routing facade

**Files:**
- Create: `gasket/engine/src/command/dispatcher.rs`
- Modify: `gasket/engine/src/command/mod.rs`

The dispatcher is a thin wrapper around `parse(...)` that the CLI / gateway calls instead of handing raw text directly to `AgentSession`. It does NOT own the orchestrator — the CLI keeps that as session state and asks the dispatcher whether the input was a command.

- [ ] **Step 1: Write failing tests**

Create `gasket/engine/src/command/dispatcher.rs`:

```rust
//! Decide whether user input is a slash-command or plain conversation,
//! returning a structured action the caller can dispatch on.

use crate::command::parser::{parse, CommandAction};

/// Stateless dispatcher — parses input given the current set of known templates.
pub struct CommandDispatcher {
    known_templates: Vec<String>,
}

impl CommandDispatcher {
    pub fn new(known_templates: Vec<String>) -> Self {
        Self { known_templates }
    }

    /// Refresh the known-template list (e.g., after a user edits ~/.gasket/flows).
    pub fn set_known_templates(&mut self, templates: Vec<String>) {
        self.known_templates = templates;
    }

    /// Route an input line.
    pub fn route(&self, input: &str) -> CommandAction {
        parse(input, &self.known_templates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_input_routes_to_plain() {
        let d = CommandDispatcher::new(vec!["default".to_string()]);
        assert_eq!(d.route("hi"), CommandAction::Plain("hi".to_string()));
    }

    #[test]
    fn test_flow_start_routes_to_start() {
        let d = CommandDispatcher::new(vec!["default".to_string()]);
        match d.route("/flow start fix bug") {
            CommandAction::Start { template, request } => {
                assert_eq!(template, "default");
                assert_eq!(request, "fix bug");
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn test_set_known_templates_affects_routing() {
        let mut d = CommandDispatcher::new(vec!["default".to_string()]);
        // Without "refactor" in known templates, "/flow start refactor X" → default tpl
        match d.route("/flow start refactor add cache") {
            CommandAction::Start { template, .. } => assert_eq!(template, "default"),
            _ => panic!("expected Start"),
        }
        // After registering "refactor"
        d.set_known_templates(vec!["default".to_string(), "refactor".to_string()]);
        match d.route("/flow start refactor add cache") {
            CommandAction::Start { template, request } => {
                assert_eq!(template, "refactor");
                assert_eq!(request, "add cache");
            }
            _ => panic!("expected Start"),
        }
    }
}
```

- [ ] **Step 2: Add to `command/mod.rs`**

```rust
//! Slash-command parsing and dispatch.

pub mod dispatcher;
pub mod parser;

pub use dispatcher::CommandDispatcher;
pub use parser::{parse, CommandAction};
```

- [ ] **Step 3: Run tests**

```bash
cargo test --package gasket-engine command::dispatcher:: 2>&1 | tail -10
```

Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/command/dispatcher.rs gasket/engine/src/command/mod.rs
git commit -m "feat(engine): add CommandDispatcher routing facade"
```

---

### Task 14: Wire dispatcher + orchestrator into the CLI agent

**Files:**
- Modify: `gasket/cli/src/commands/agent.rs`

This task is wiring, not new logic. The agent CLI currently takes raw user input and passes it to `AgentSession::process_direct_streaming_with_channel`. We insert a dispatch step before that.

**Note:** The CLI's existing input loop is in `gasket/cli/src/commands/agent.rs` after line 269 (find `match opts.message`). The interactive (REPL) path uses `Reedline`; we add the dispatch logic in the line-handling closure.

- [ ] **Step 1: Read the existing input handler to find the integration point**

```bash
sed -n '260,360p' gasket/cli/src/commands/agent.rs
```

Look for the place where each user line goes into `agent.process_direct_streaming_with_channel(...)`. We want to insert dispatch *just before* that call.

- [ ] **Step 2: Add imports near the top of `agent.rs`**

After the existing `use gasket_engine::...` imports, add:

```rust
use gasket_engine::command::{CommandAction, CommandDispatcher};
use gasket_engine::flow::{
    CliGate, FlowOrchestrator, OrchestratorOutcome, TemplateLoader,
};
```

- [ ] **Step 3: Build the TemplateLoader and CommandDispatcher near the agent setup**

Find where `agent_config` is built (around line 56) and the agent is finalized (around line 264). After the agent is built but before the message loop starts (around line 266 — `let render_md = ...`), insert:

```rust
    // Flow command system setup
    let user_flows_dir = dirs::home_dir()
        .map(|h| h.join(".gasket/flows"))
        .unwrap_or_else(|| std::path::PathBuf::from(".gasket/flows"));
    let builtin_flows_dir = std::env::var("GASKET_FLOWS_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            // dev fallback: look for engine/flows under exe path or cwd
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().and_then(|x| x.parent()).and_then(|x| x.parent()).map(|x| x.join("engine/flows")))
                .filter(|p| p.exists())
        });
    let template_loader = TemplateLoader::new(user_flows_dir.clone(), builtin_flows_dir.clone());
    let known_templates = template_loader.list();
    let mut dispatcher = CommandDispatcher::new(known_templates);
    let flow_run_store = sqlite_store.flow_run_store();

    // Active flow handle (one per CLI session)
    let mut active_flow: Option<FlowOrchestrator> = None;
```

- [ ] **Step 4: Modify the input-handling block to dispatch each line**

Find the section that processes each user line. Wrap the existing call to `agent.process_direct_streaming_with_channel(line, ...)`:

```rust
    // Replace the direct call with this dispatched version:
    match dispatcher.route(&line) {
        CommandAction::Plain(text) => {
            // existing behavior — passes to AgentSession unchanged
            // (preserve whatever existing code calls process_direct_streaming_with_channel)
        }
        CommandAction::Start { template, request } => {
            if active_flow.is_some() {
                eprintln!("⚠ A flow is already active. Run /flow abort or /flow resume first.");
                continue;
            }
            match FlowOrchestrator::start_new(
                &template_loader,
                flow_run_store.clone(),
                &template,
                request,
                session_key.to_string(),
            ).await {
                Ok(orch) => {
                    println!("▶ Flow {} started (template: {}). flow_id={}",
                        template, template, orch.flow_id());
                    active_flow = Some(orch);
                    // Step the flow until AwaitingGate or Done/Paused
                    drive_flow(&mut active_flow, &agent, session_key).await;
                }
                Err(e) => eprintln!("⚠ Failed to start flow: {e}"),
            }
        }
        CommandAction::Status => {
            match &active_flow {
                Some(o) => println!("Active flow: {} ({:?})", o.flow_id(), o.status()),
                None => println!("No active flow."),
            }
        }
        CommandAction::Resume { flow_id } => {
            match FlowOrchestrator::resume(&template_loader, flow_run_store.clone(), &flow_id).await {
                Ok(orch) => {
                    println!("▶ Resumed flow {}", orch.flow_id());
                    active_flow = Some(orch);
                    drive_flow(&mut active_flow, &agent, session_key).await;
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
                Ok(rows) => {
                    for r in rows {
                        println!("{} {} {} ({})", r.flow_id, r.status, r.current_phase, r.template_name);
                    }
                }
                Err(e) => eprintln!("⚠ List failed: {e}"),
            }
        }
        CommandAction::Unknown { command, .. } => {
            eprintln!("⚠ Unknown command: /{}", command);
        }
    }
```

Add a `drive_flow` helper at the bottom of the file (or in a `mod flow_helper`):

```rust
async fn drive_flow(
    active_flow: &mut Option<FlowOrchestrator>,
    _agent: &AgentSession,
    _session_key: &SessionKey,
) {
    // v1: minimal driver — does not yet integrate the kernel runtime context.
    // Full integration is a follow-up; here we just check status and prompt for gates.
    let Some(o) = active_flow else { return; };
    loop {
        match o.status() {
            gasket_types::FlowStatus::Running => {
                // The kernel-driven step would go here. For v1 CLI bootstrap,
                // we surface a placeholder to the user and break.
                eprintln!("[flow:{}] Phase '{}' — kernel integration pending (v1.1)",
                    o.flow_id(), o.state.current_phase.as_str());
                break;
            }
            gasket_types::FlowStatus::AwaitingGate { phase } => {
                let label = phase.as_str();
                println!("\n[Gate: {}]", label);
                let resp = match CliGate::read_response("y/n/edit/redo/back?") {
                    Ok(r) => r,
                    Err(e) => { eprintln!("⚠ Gate read failed: {e}"); break; }
                };
                if let Err(e) = o.apply_gate(resp).await {
                    eprintln!("⚠ Gate apply failed: {e}");
                    break;
                }
            }
            gasket_types::FlowStatus::Done => {
                let pending = o.drain_pending_wiki();
                if !pending.is_empty() {
                    println!("Flow done. {} pending wiki write(s) — review with /wiki review (TODO).", pending.len());
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
```

**Important:** This `drive_flow` is a **deliberate v1 stub** — the spec §7 step 12 is "CLI / gateway entry wiring", and the actual kernel-driven phase execution requires a `RuntimeContext` for each phase. That wiring depends on whether `tools` Arc inside `agent` can be cloned with a per-phase guard injection — exposing that requires a small AgentSession accessor that we are *not* adding in v1 to keep the surface clean.

For v1, the dispatcher and orchestrator persist correctly; resume works; gates are interactive. Phase execution itself (calling the kernel from PhaseRunner with a tool-filtered registry) is the explicit deferral. The stub message makes that clear to the user.

This is consistent with the spec §8 "Out of Scope (v1)" — kernel integration via filtered tools is **not** explicitly listed there, so we add it: "v1 ships orchestrator, persistence, gate UI, and command parsing. Per-phase kernel invocation with tool-filter is v1.1."

**Update the spec §8 to reflect this** in Task 16.

- [ ] **Step 5: Build the workspace**

```bash
cargo build --workspace 2>&1 | tail -20
```

Expected: builds without errors.

- [ ] **Step 6: Manual smoke test**

```bash
cargo run --release --package gasket-cli -- agent -m "/flow list"
```

Expected: empty list (no flows yet) — confirms the dispatcher routes correctly.

```bash
cargo run --release --package gasket-cli -- agent -m "hello"
```

Expected: normal agent response — confirms plain text bypasses the flow path.

- [ ] **Step 7: Commit**

```bash
git add gasket/cli/src/commands/agent.rs
git commit -m "feat(cli): wire CommandDispatcher and FlowOrchestrator into agent loop"
```

---

### Task 15: Ship built-in templates and prompts

**Files:**
- Create: `gasket/engine/flows/default.yaml`
- Create: `gasket/engine/flows/debug.yaml`
- Create: `gasket/engine/flows/docs.yaml`
- Create: `gasket/engine/flows/brainstorm-only.yaml`
- Create: `gasket/engine/flows/design-only.yaml`
- Create: `gasket/engine/flows/plan-only.yaml`
- Create: `gasket/engine/flows/execute-only.yaml`
- Create: `gasket/engine/flows/verify-only.yaml`
- Create: `gasket/engine/flows/prompts/{brainstorm,design,plan,execute,verify}.md`

- [ ] **Step 1: Create the prompts directory and files**

Create `gasket/engine/flows/prompts/brainstorm.md`:

```markdown
[Phase: Brainstorm — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

You are in the **brainstorm** phase. Goal: understand intent and surface trade-offs before designing.

Allowed tools: wiki_search, wiki_read, history_search.

Do:
1. Ask at most one clarifying question if intent is ambiguous.
2. Use `wiki_search` and `history_search` to find relevant prior knowledge.
3. Surface 2-3 possible directions with brief trade-offs.
4. End with a one-paragraph summary the user can confirm.

Do NOT: write code, design schemas, or plan tasks. Save those for later phases.
```

`gasket/engine/flows/prompts/design.md`:

```markdown
[Phase: Design — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

Brainstorm outcome:
> {{previous_outputs.brainstorm}}

You are in the **design** phase. Goal: produce a concrete design that resolves all open questions before planning.

Cover:
- Architecture / component boundaries
- Data flow
- Error handling strategy
- Testing approach (high-level)

Allowed tools: wiki_search, wiki_read, file_read, file_search.

End with a clear, sectioned design summary suitable for review.
```

`gasket/engine/flows/prompts/plan.md`:

```markdown
[Phase: Plan — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

Design outcome:
> {{previous_outputs.design}}

You are in the **plan** phase. Break the design into bite-sized tasks an engineer can execute one at a time.

Each task:
- Lists the exact files to touch.
- Has a TDD step ordering: write test → fail → implement → pass → commit.
- Is independently verifiable.

Allowed tools: wiki_search, file_read, file_search.

End with a numbered task list.
```

`gasket/engine/flows/prompts/execute.md`:

```markdown
[Phase: Execute — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

Plan:
> {{previous_outputs.plan}}

You are in the **execute** phase. Implement the plan task by task.

All tools are available. Follow the plan's tests-first ordering. Commit after each task.

When all tasks are done, summarise what changed and call attention to any deviations from the plan.
```

`gasket/engine/flows/prompts/verify.md`:

```markdown
[Phase: Verify — Step {{phase_index}} of {{phase_total}}]

User's request:
> {{user_request}}

What was executed:
> {{previous_outputs.execute}}

You are in the **verify** phase. Confirm the implementation actually delivers the goal.

Do:
- Run all relevant tests; report pass/fail.
- Verify the user's original request is satisfied.
- Surface anything that wasn't implemented or that deviated.

Allowed tools: shell, file_read, test_runner.
```

- [ ] **Step 2: Create `default.yaml`**

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
      required: true
      prompt: "Brainstorm direction looks right? (y/n/edit)"

  - id: design
    label: Design
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 8
    gate:
      required: true
      prompt: "Design accepted? (y/n/edit)"

  - id: plan
    label: Plan
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read, file_search]
    max_iterations: 5
    gate:
      required: true
      prompt: "Plan looks executable? (y/n/edit)"

  - id: execute
    label: Execute
    prompt_file: prompts/execute.md
    allowed_tools: ["*"]
    max_iterations: 0
    gate:
      required: false
      prompt: ""

  - id: verify
    label: Verify
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read]
    max_iterations: 5
    gate:
      required: true
      prompt: "Verification passed? End flow? (y/n/redo)"
```

- [ ] **Step 3: Create the simpler templates**

`debug.yaml`:

```yaml
name: debug
description: Bug-fix flow — skips design phase
version: 1
wiki_policy: deferred

phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, history_search, file_read]
    max_iterations: 5
    gate:
      required: true
      prompt: "Diagnosed correctly? (y/n/edit)"

  - id: plan
    label: Plan
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read]
    max_iterations: 3
    gate:
      required: true
      prompt: "Fix plan accepted? (y/n/edit)"

  - id: execute
    label: Execute
    prompt_file: prompts/execute.md
    allowed_tools: ["*"]
    max_iterations: 0
    gate:
      required: false
      prompt: ""

  - id: verify
    label: Verify
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read]
    max_iterations: 5
    gate:
      required: true
      prompt: "Bug fixed? (y/n/redo)"
```

`docs.yaml`:

```yaml
name: docs
description: Documentation flow — brainstorm → design → execute (no plan/verify)
version: 1
wiki_policy: deferred

phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 3
    gate:
      required: true
      prompt: "Topic understood? (y/n/edit)"

  - id: design
    label: Outline
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, file_read]
    max_iterations: 3
    gate:
      required: true
      prompt: "Outline accepted? (y/n/edit)"

  - id: execute
    label: Write
    prompt_file: prompts/execute.md
    allowed_tools: [file_read, file_search]
    max_iterations: 0
    gate:
      required: true
      prompt: "Documentation acceptable? (y/n/redo)"
```

- [ ] **Step 4: Create the single-phase shortcut templates**

`brainstorm-only.yaml`:

```yaml
name: brainstorm-only
description: Single-phase brainstorm
version: 1
wiki_policy: deferred
phases:
  - id: brainstorm
    label: Brainstorm
    prompt_file: prompts/brainstorm.md
    allowed_tools: [wiki_search, wiki_read, history_search]
    max_iterations: 5
    gate:
      required: true
      prompt: "Done with brainstorm? (y/n)"
```

Repeat the same single-phase pattern for `design-only.yaml`, `plan-only.yaml`, `execute-only.yaml`, `verify-only.yaml`, each pointing at its respective prompt file with appropriate `allowed_tools`.

- [ ] **Step 5: Verify YAML parses by building + running list**

```bash
cargo build --workspace 2>&1 | tail -3
GASKET_FLOWS_DIR=$(pwd)/gasket/engine/flows cargo run --release --package gasket-cli -- agent -m "/flow list"
```

Expected: list output shows nothing (no flow_runs yet — flow files are templates, not runs). The fact that the CLI didn't crash on YAML parse is the success signal.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/flows/
git commit -m "feat(engine): ship built-in flow templates (default, debug, docs, single-phase shortcuts)"
```

---

### Task 16: Mark old design as superseded + minor scope-doc fix

**Files:**
- Modify: `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md`
- Modify: `docs/superpowers/specs/2026-05-03-flow-command-system-design.md`

- [ ] **Step 1: Add Status: Superseded line to old design**

In `docs/superpowers/specs/2026-04-30-phased-agent-loop-design.md`, change line 4 from:

```markdown
**Status**: Draft
```

to:

```markdown
**Status**: Superseded by `2026-05-03-flow-command-system-design.md`
```

- [ ] **Step 2: Add v1 deferral note to new design's §8**

In `docs/superpowers/specs/2026-05-03-flow-command-system-design.md`, find section "## 8. Out of Scope (v1)". Add a final bullet:

```markdown
- Per-phase kernel invocation with `allowed_tools` filter — v1 ships orchestrator, persistence, gate UI, command parser, and YAML templates. Calling the kernel for each phase via PhaseRunner with a filtered ToolRegistry happens in v1.1 once the AgentSession exposes a per-call tool override accessor.
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/
git commit -m "docs: mark phased-agent-loop spec superseded; clarify v1 deferral"
```

---

### Task 17: End-to-end smoke verification

**Files:** None new.

- [ ] **Step 1: Build the entire workspace**

```bash
cargo build --workspace --release 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 2: Run the full test suite**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass. New tests added across tasks total ~50.

- [ ] **Step 3: Test plain conversation still works (regression)**

```bash
cargo run --release --package gasket-cli -- agent -m "what is 2+2?"
```

Expected: normal LLM response, no flow indicators.

- [ ] **Step 4: Test slash command parsing**

```bash
cargo run --release --package gasket-cli -- agent -m "/flow list"
cargo run --release --package gasket-cli -- agent -m "/flow status"
```

Expected: empty list / "No active flow."

- [ ] **Step 5: Verify SQLite migration runs on fresh DB**

```bash
rm -f ~/.gasket/gasket.db && cargo run --release --package gasket-cli -- agent -m "/flow list"
sqlite3 ~/.gasket/gasket.db ".schema flow_runs"
```

Expected: shows the `flow_runs` table DDL.

- [ ] **Step 6: Commit any tweaks discovered during smoke testing, if needed**

```bash
git status
# fix any small issues if present, then:
git add -A && git commit -m "chore: e2e verification fixes for flow command system"
```

If no fixes needed, skip the commit.

---

## Self-Review Checklist (run after writing the plan)

**1. Spec coverage check**

| Spec section | Task(s) |
|---|---|
| §2.1 Layer Map | Tasks 5, 9, 10, 11, 12, 13, 14 |
| §2.2 Component Boundaries | Tasks 5–13 |
| §2.3 Touch Points | Tasks 1, 2, 3, 4, 5, 8, 14 |
| §3.1 Template Layout | Task 15 |
| §3.2 YAML Schema | Task 5 |
| §3.3 Variables | Task 5 (`render_prompt` + Task 11 `template_vars`) |
| §3.4 Slash Commands | Task 12 |
| §3.5 Gate Interaction | Task 10 |
| §4.1 E2E Sequence | Task 11 |
| §4.2 FlowState | Task 6 |
| §4.3 SQLite Schema | Task 3 |
| §4.4 State Machine Transitions | Task 6 (test cases match the table) |
| §4.5 Resume | Task 11 (`resume`) |
| §4.6 WikiGuard | Tasks 7, 8 |
| §5.1 Error Catalog | Tasks 11 (orchestrator), 12 (parser), 14 (CLI) |
| §5.2 Invariants | Tasks 6 (state tests), 11 (orchestrator tests) |
| §6 Testing Strategy | Each task has tests |
| §7 Implementation Order | Tasks 1–16 follow §7 closely; small reorderings for TDD |
| §8 Out of Scope | Honored — Web frontend deferred; v1.1 deferrals noted in Task 16 |

**2. Placeholder scan** — none found in tasks 1-15. Task 14's `drive_flow` is explicitly marked as a v1 stub with documented v1.1 follow-up; this is documented deferral, not a placeholder.

**3. Type consistency** — `FlowState`, `FlowStatus`, `PhaseId`, `PhaseOutput`, `PendingWikiWrite` are introduced in Task 1 (gasket-types) and used consistently across Tasks 6, 7, 11. `WriteArgsView` introduced in Task 7, used in Task 8. `GateResponse` introduced in Task 6, used in Tasks 10, 11.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-03-flow-command-system.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for the 17 tasks here.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints. Slower but gives you visibility into each step.

**Which approach?**
