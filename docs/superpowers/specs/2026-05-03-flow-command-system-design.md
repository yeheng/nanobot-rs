# Flow Command System — Design Spec

**Date**: 2026-05-03
**Status**: Draft
**Supersedes**: `2026-04-30-phased-agent-loop-design.md`
**Scope**: Slash-command-gated, YAML-templated, persistable plan-act-review flow engine for complex tasks

---

## 1. Overview

Replace the always-on phased agent loop with a **command-gated flow engine**. Plain conversation continues to use the existing `AgentSession` path unchanged. When the user enters a slash command (`/flow start`, `/design`, `/brainstorm`, etc.), a new `FlowOrchestrator` takes over and drives a YAML-defined sequence of phases with user-confirm gates at each boundary.

The default flow has five phases — `brainstorm → design → plan → execute → verify` — each with its own prompt, allowed tool subset, iteration cap, and optional review gate. Templates are stored as YAML files in `~/.gasket/flows/` (user-level) and `gasket/engine/flows/` (built-in fallbacks). Phase-boundary snapshots in a new `flow_runs` SQLite table enable `/flow resume`. Wiki writes attempted during a flow are intercepted and only committed at flow end with explicit user approval, eliminating the current over-write problem.

### Key Differences vs. Superseded Design

| Dimension | Old (`phased-agent-loop`) | New (this spec) |
|---|---|---|
| Entry | Config flag, every user message goes phased | Slash command only — plain text bypasses |
| Phases | Hard-coded 4 (Research/Planning/Execute/Review) | YAML-defined, default 5 (Brainstorm/Design/Plan/Execute/Verify) |
| Customization | None | Per-phase prompt, tools, max_iter, gate; multi-template |
| Review/wiki write | LLM-driven `wiki_write` in Review phase | Deferred queue, end-of-flow user confirmation |
| Persistence | None | `flow_runs` table, phase-boundary snapshots, resume |
| Gate | None | Explicit per-phase user confirm with edit/redo/back |

---

## 2. Architecture

### 2.1 Layer Map

```
┌────────────────────────────────────────────────────────────────┐
│  Channels (CLI / Web / Telegram / ...)                         │
│      ↓ user input text                                         │
├────────────────────────────────────────────────────────────────┤
│  ★ NEW: engine/command/                                         │
│  CommandDispatcher                                              │
│    ├─ parse first token: leading "/"?                           │
│    ├─ command match → FlowOrchestrator                          │
│    └─ no match / plain text → AgentSession (existing path)      │
├────────────────────────────────────────────────────────────────┤
│  ★ NEW: engine/flow/                                            │
│  FlowOrchestrator         (state machine, owns one flow run)    │
│    ├─ TemplateLoader      (YAML → FlowTemplate)                 │
│    ├─ FlowState           (current_phase, prev_outputs, ...)    │
│    ├─ PhaseRunner         (drives one phase via kernel)         │
│    ├─ GateController      (user-confirm at phase boundaries)    │
│    └─ WikiGuard           (intercepts wiki_write tool calls)    │
├────────────────────────────────────────────────────────────────┤
│  AgentSession (existing, unmodified)                            │
│    └─ kernel::execute_streaming()  ← PhaseRunner calls into     │
├────────────────────────────────────────────────────────────────┤
│  Storage                                                        │
│    └─ ★ NEW table: flow_runs (snapshots per phase boundary)     │
└────────────────────────────────────────────────────────────────┘
```

### 2.2 Component Boundaries

| Component | Owns | Knows | Doesn't know |
|---|---|---|---|
| `CommandDispatcher` | Command grammar, routing | How to dispatch | Flow internals, LLM |
| `FlowOrchestrator` | YAML templates, phase state, user gates | Lifecycle, persistence | Provider, low-level storage |
| `PhaseRunner` | One phase's prompt + tool filter + kernel call | How to drive a single phase | Cross-phase logic |
| `GateController` | User confirmation interaction | gate prompt, response parsing | LLM, state machine |
| `WikiGuard` | Interception rules, pending queue | Whether to allow/defer/reject | wiki write internals |
| `AgentSession` / `kernel` | LLM loop, tool execution | Existing behavior | Flow's existence |

### 2.3 Touch Points with Existing Code

1. `gasket/engine/src/lib.rs` — add `pub mod command;` and `pub mod flow;`
2. CLI entry (`gasket/cli/src/commands/agent.rs`) and gateway entry — route raw user input through `CommandDispatcher::route()` first
3. `gasket/engine/src/tools/wiki_tools.rs::WikiWriteTool` — accept an injectable `Arc<dyn WikiWriteGuard>`; default impl is `AllowAllGuard`
4. `gasket/storage/src/` — add `flow_run_store.rs` and a migration in `migrations/`
5. `gasket/types/src/events/stream.rs` — add `FlowStarted`, `PhaseChanged`, `GatePending`, `FlowFinished` `ChatEvent` variants

No changes to `kernel/`, `bus/`, `providers/`, `channels/` impl crates.

---

## 3. Templates and Commands

### 3.1 Template File Layout

```
~/.gasket/flows/                  # user-level templates (higher priority)
├── new-feature.yaml
├── refactor.yaml
└── quick-fix.yaml

gasket/engine/flows/              # built-in defaults (fallback)
├── default.yaml                  # brainstorm→design→plan→execute→verify
├── debug.yaml                    # brainstorm→plan→execute→verify (skip design)
└── docs.yaml                     # brainstorm→design→execute (skip plan/verify)
```

Loading order: user dir overrides built-in by name. Mirrors the existing `skills/` system convention.

Built-in templates and their `prompt_file` references are shipped inside the `gasket/engine/flows/` source directory. At runtime the engine resolves a template's prompt files relative to the template YAML's own location — the same mechanism for both built-in and user templates. The `GASKET_FLOWS_DIR` environment variable can override the built-in directory (parallel to `GASKET_SKILLS_DIR`).

### 3.2 Template YAML Schema

```yaml
# ~/.gasket/flows/new-feature.yaml
name: new-feature
description: Full new-feature workflow with design + plan review gates
version: 1

# Global policy
wiki_policy: deferred          # deferred | blocked | allowed
                               # deferred = intercept during flow, ask user at end (default)

phases:
  - id: brainstorm
    label: "Brainstorm"
    prompt_file: prompts/brainstorm.md      # path relative to template file's dir
    allowed_tools: [wiki_search, wiki_read, history_search]
    max_iterations: 5
    gate:
      required: true
      prompt: "Brainstorm direction looks right? (y/n/edit)"

  - id: design
    label: "Design"
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 8
    gate:
      required: true
      prompt: "Design accepted? (y/n/edit)"

  - id: plan
    label: "Plan"
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read, file_search]
    max_iterations: 5
    gate:
      required: true
      prompt: "Plan looks executable? (y/n/edit)"

  - id: execute
    label: "Execute"
    prompt_file: prompts/execute.md
    allowed_tools: ["*"]        # full tool set
    max_iterations: 0           # 0 means unlimited; positive integer is the cap
    gate:
      required: false           # auto-advance to verify

  - id: verify
    label: "Verify"
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read, test_runner]
    max_iterations: 5
    gate:
      required: true
      prompt: "Verification passed? End flow? (y/n/redo-execute)"
```

### 3.3 Prompt Template Variables

Each `prompt_file` may use mustache-style placeholders:

| Variable | Meaning |
|---|---|
| `{{user_request}}` | The original user request text |
| `{{prev_phase_output}}` | LLM's final summary text from the previous phase |
| `{{flow_id}}` | UUID of the current flow |
| `{{phase_index}}` / `{{phase_total}}` | Progress indicator |
| `{{previous_outputs.<phase_id>}}` | Any completed phase's output, indexed by id |

### 3.4 Slash Command Set

| Command | Behavior |
|---|---|
| `/flow start <template> -- <request>` | Start flow with named template; `--` separates template name from free-form request |
| `/flow start <request>` | Use `default.yaml`. Disambiguation: if no `--` is present and the first token does **not** match a known template name, the entire arg string is treated as the request |
| `/flow status` | Show current active flow's phase + progress |
| `/flow resume <flow_id>` | Resume from last snapshot |
| `/flow abort` | Terminate current flow (snapshot retained 30 days) |
| `/flow list` | List recent flow_runs with status |
| `/brainstorm <request>` | Shortcut: starts a single-phase brainstorm-only flow |
| `/design`, `/plan`, `/execute`, `/verify` | Same single-phase shortcuts |
| Anything not starting with `/` | Bypass — goes to `AgentSession` (plain conversation) |

### 3.5 Gate Interaction

When `gate.required: true` after a phase's `kernel` call returns:

```
[Gate: design]
LLM summary:
> <last LLM response content>

Input:
  y / yes      → accept, advance to next phase
  n / no       → abort flow, snapshot retained
  edit         → enter feedback text, stay in current phase, iterate
  redo         → re-run current phase from scratch (clear current phase context)
  back         → go back to previous phase's gate
```

CLI: stdin readline. Web: `gate_pending` event + button selection sent back via WebSocket.

---

## 4. Data Flow and State Machine

### 4.1 End-to-End Sequence

```
User: "/flow start new-feature add auth to wiki"
   ▼
[CommandDispatcher::route()]
   parse → FlowAction::Start { template: "new-feature", request: "..." }
   ▼
[FlowOrchestrator::start_new(template, request)]
   1. Load template (~/.gasket/flows/new-feature.yaml)
   2. Generate flow_id (UUID v7)
   3. INSERT INTO flow_runs (flow_id, template_name, status='running',
                             current_phase='brainstorm', user_request, ...)
   4. Init FlowState
   ▼
[PhaseRunner::run(phase=brainstorm)]
   1. Load prompts/brainstorm.md, render variables
   2. Build RuntimeContext for kernel:
        - tool_filter = ["wiki_search", "wiki_read", "history_search"]
        - WikiGuard pre-injected on the wiki_write tool
   3. Call kernel::execute_streaming(ctx, messages, event_tx)
        - Stream output via channels (existing path)
        - Get ExecutionResult
   ▼
[GateController::wait_for_user(phase=brainstorm)]
   1. Show result.content as phase summary
   2. Show gate prompt
   3. Block waiting on user (CLI: stdin / Web: gate_pending event)
   ▼
[Snapshot]
   UPDATE flow_runs SET
     current_phase='design',
     completed_phases = json_set(completed_phases,
        '$.brainstorm', json('{...output...}')),
     updated_at = NOW()
   WHERE flow_id = ?
   ▼
... repeat design / plan / execute / verify ...
   ▼
[verify gate passed]
   ▼
[FlowOrchestrator::finalize()]
   1. UPDATE flow_runs SET status='done'
   2. Inspect WikiGuard pending queue (all wiki_write args from this flow)
   3. If non-empty → ask user:
      "N wiki pages were proposed. Choose:
        all = write all / pick = pick one by one / none = discard / show = view full"
   4. On selection → call PageStore.write() for chosen entries
   ▼
[Done]
```

### 4.2 FlowState (runtime)

```rust
// engine/src/flow/state.rs
pub struct FlowState {
    pub flow_id: Uuid,                    // v7 (time-ordered)
    pub template: Arc<FlowTemplate>,      // loaded YAML
    pub user_request: String,
    pub current_phase: PhaseId,
    pub status: FlowStatus,
    pub completed_phases: BTreeMap<PhaseId, PhaseOutput>,
    pub pending_wiki_writes: Vec<PendingWikiWrite>,
    pub session_key: SessionKey,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum PhaseId { Brainstorm, Design, Plan, Execute, Verify, Custom(String) }

pub enum FlowStatus {
    Running,
    AwaitingGate { phase: PhaseId },
    Paused,
    Done,
    Aborted,
}

pub struct PhaseOutput {
    pub summary: String,                  // final LLM text at phase end
    pub iterations_used: u32,
    pub tools_called: Vec<String>,
    pub finished_at: DateTime<Utc>,
}
```

### 4.3 SQLite Schema

A new `flow_run` migration is added under `gasket/storage/src/migrations/` (Rust module, mirroring existing `cron.rs` / `kv.rs` / `session.rs`):

```sql
-- DDL emitted by gasket/storage/src/migrations/flow_run.rs
CREATE TABLE IF NOT EXISTS flow_runs (
  flow_id            TEXT PRIMARY KEY,         -- UUID v7
  template_name      TEXT NOT NULL,
  template_version   INTEGER NOT NULL,
  user_request       TEXT NOT NULL,
  session_key        TEXT NOT NULL,
  status             TEXT NOT NULL,            -- running|awaiting_gate|paused|done|aborted
  current_phase      TEXT NOT NULL,
  completed_phases   TEXT NOT NULL DEFAULT '{}', -- JSON: {phase_id: PhaseOutput}
  pending_wiki       TEXT NOT NULL DEFAULT '[]', -- JSON: [PendingWikiWrite]
  created_at         INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL
);

CREATE INDEX idx_flow_runs_session ON flow_runs(session_key, updated_at DESC);
CREATE INDEX idx_flow_runs_status ON flow_runs(status, updated_at DESC);
```

### 4.4 State Machine Transitions

| From `status` | Trigger | To `status` |
|---|---|---|
| `Running` | PhaseRunner finished, `gate.required=true` | `AwaitingGate{phase}` |
| `Running` | PhaseRunner finished, `gate.required=false` | `Running` (auto-advance) |
| `Running` | PhaseRunner finished, last phase | `Done` |
| `Running` | PhaseRunner errored | `Paused` |
| `AwaitingGate` | user input `y` | `Running` (advance) |
| `AwaitingGate` | user input `n` | `Aborted` |
| `AwaitingGate` | user input `edit <text>` | `Running` (iterate same phase, inject text) |
| `AwaitingGate` | user input `redo` | `Running` (clear phase output, re-run) |
| `AwaitingGate` | user input `back` | `AwaitingGate{prev_phase}` |
| `Paused` / `AwaitingGate` | `/flow resume <id>` | restore prior status |
| any non-`Done` | `/flow abort` | `Aborted` |

### 4.5 Resume Behavior

`/flow resume <flow_id>`:
1. `SELECT * FROM flow_runs WHERE flow_id = ?`
2. Rebuild `FlowState` (template reloaded — may have changed)
3. If `status = AwaitingGate` → re-print phase summary + gate prompt
4. If `status = Paused` and `current_phase = X` → re-run phase X from scratch (no step-level snapshot — this is the explicit trade-off chosen with phase-boundary granularity)

### 4.6 WikiGuard

```rust
// engine/src/flow/wiki_guard.rs

#[async_trait]
pub trait WikiWriteGuard: Send + Sync {
    async fn intercept(&self, args: WriteArgs) -> GuardDecision;
}

pub enum GuardDecision {
    Allow,                    // pass through (no flow / wiki_policy=allowed)
    Defer(WriteArgs),         // queue into FlowState.pending_wiki_writes
    Reject(String),           // reject, return error string to LLM
}

pub struct DeferringGuard {
    pending: Arc<Mutex<Vec<PendingWikiWrite>>>,
}
pub struct AllowAllGuard;
pub struct BlockingGuard;
```

`WikiWriteTool::execute` first line: `match guard.intercept(args).await { ... }`. The plain-conversation path uses `AllowAllGuard`, so behavior outside flows is unchanged.

---

## 5. Error Handling and Invariants

### 5.1 Error Catalog

| Source | Handling |
|---|---|
| YAML parse failure | `/flow start` returns error; no row inserted |
| Missing `prompt_file` referenced by template | Same as above |
| Kernel error inside a phase (provider/network/max_iter) | Flow → `Paused`; return error + flow_id; suggest `/flow resume` |
| Channel disconnect during gate wait | Flow → `Paused` |
| WikiGuard SQLite error queueing pending write | Degrade to `Reject(str)` returned to LLM; flow continues; warn log |
| User runs `/flow start` with active flow already | Reject with "active flow exists, abort or resume first" |
| Resume with non-existent flow_id | Error + list 5 most recent flows |
| Resume with deleted/changed template | Warn user, continue with current template; if template missing → abort |
| LLM calls a tool not in the phase's `allowed_tools` | ToolRegistry filtered set returns standard `ToolError::ToolNotFound` to the LLM |

### 5.2 Invariants

1. A session has at most one flow_run with `status ∈ {Running, AwaitingGate, Paused}` at any time.
2. `current_phase` must be an id present in `template.phases`.
3. Keys of `completed_phases` are a subset of `template.phases` ids that come before `current_phase`.
4. `status = Done` ⟺ `current_phase` is the last phase in `template.phases` AND its gate has passed.
5. `pending_wiki_writes` is non-empty only when `wiki_policy = deferred`.
6. LLM tool calls during a phase are restricted to that phase's `allowed_tools` (or all tools if `["*"]`).

These should be encoded as `debug_assert!` in dev builds and as test assertions in integration tests.

---

## 6. Testing Strategy

| Level | Component | Path | Key cases |
|---|---|---|---|
| Unit | `TemplateLoader` | `flow/template_test.rs` | valid/invalid YAML, user override built-in |
| Unit | State machine | `flow/state_test.rs` | All §4.4 transitions exhaustively |
| Unit | `WikiGuard` | `flow/wiki_guard_test.rs` | Allow / Defer / Reject branches |
| Unit | `CommandDispatcher` parse | `command/parser_test.rs` | `/flow start ...`, shortcuts, quoted args, unknown `/cmd`, plain text |
| Integration | `flow_run_store` | `storage/flow_run_store_test.rs` | insert/update/load/list, JSON round-trip |
| Integration | E2E flow with mock provider | `flow/orchestrator_test.rs` | scripted 5-phase run with programmatic gate inputs |
| Integration | Resume | `flow/orchestrator_test.rs` | mid-run "crash" → restart → resume → completion |
| Integration | Wiki interception | `flow/wiki_intercept_test.rs` | flow buffers writes; finalize commits chosen ones |
| Regression | Plain conversation unchanged | existing test suites | non-`/` input takes existing path verbatim |

### 6.1 Mock Provider Helper

```rust
let provider = MockLlmProvider::new()
    .respond("brainstorm: I suggest... [phase summary]")  // brainstorm iter 1
    // gate y
    .respond("design: proposal...")                        // design iter 1
    // gate y
    .respond_with_tool("plan", "wiki_search", json!({...}))
    .respond("plan: here is the plan...")                  // plan iter 2
    // gate y
    .respond_with_tool("execute", "shell", json!({"cmd":"..."}))
    .respond("execute: done")
    // auto-advance to verify (gate.required=false)
    .respond("verify: tests pass")
    // gate y
    .build();
```

### 6.2 Observability

- Each phase enter/exit and gate trigger emits a structured `tracing::info!` (`flow_id`, `phase`, `iterations`, `tools`, `duration_ms`).
- New `ChatEvent` variants: `FlowStarted{flow_id, template}`, `PhaseChanged{phase}`, `GatePending{phase, prompt}`, `FlowFinished{flow_id, status}`. Web UI subscribes for phase progress + gate buttons. CLI uses simple ANSI banners: `▶ [design] (2/5)`.

---

## 7. Implementation Order

1. **`gasket/types/src/flow.rs`** — `FlowStatus`, `PhaseId`, `PhaseOutput`, `PendingWikiWrite` types
2. **`gasket/types/src/events/stream.rs`** — new `ChatEvent` variants
3. **`gasket/storage/src/flow_run_store.rs`** + migration — CRUD for `flow_runs` table
4. **`gasket/engine/src/flow/template.rs`** — YAML loader, default `default.yaml` shipped
5. **`gasket/engine/src/flow/state.rs`** — `FlowState` + state machine transitions (pure)
6. **`gasket/engine/src/flow/wiki_guard.rs`** — `WikiWriteGuard` trait + 3 impls
7. **`gasket/engine/src/tools/wiki_tools.rs`** — wire `WikiWriteGuard` into `WikiWriteTool`
8. **`gasket/engine/src/flow/phase_runner.rs`** — single-phase kernel call wrapper
9. **`gasket/engine/src/flow/gate.rs`** — gate controller (CLI + Web variants)
10. **`gasket/engine/src/flow/orchestrator.rs`** — `FlowOrchestrator` tying it all together
11. **`gasket/engine/src/command/`** — dispatcher + parser
12. **CLI / gateway entry wiring** — route through dispatcher
13. **Web frontend** — handle new events, render gate UI
14. **Built-in templates** — ship `default.yaml`, `debug.yaml`, `docs.yaml` with prompts
15. **Documentation** — user guide for writing custom templates

---

## 8. Out of Scope (v1)

- Conditional phase branching ("if execute fails go back to plan"). Templates are linear in v1.
- Step-level resume (only phase-boundary).
- Concurrent flows in a single session (one active flow per session).
- Multi-user gate approval.
- Web UI gate buttons polish — events emitted v1, full UI may be v1.1.
- Editing a running flow's template hot. Templates load at flow start; resume reloads.
