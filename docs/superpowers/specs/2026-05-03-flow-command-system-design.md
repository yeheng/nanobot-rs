# Flow Command System — Design Spec

**Date**: 2026-05-03 (revised 2026-05-04 after Linus review)
**Status**: Revised — ready for implementation
**Supersedes**: `2026-04-30-phased-agent-loop-design.md`
**Scope**: Two-batch delivery — (v1) wiki-write deferred queue, (v2) slash-command-gated YAML flow engine on top of v1

---

## 1. Overview

Two delivery batches with independent shippable value:

**Batch v1 — Wiki Write Guard** (the user's most acute pain point):
A `WikiGuard` sits in front of `WikiWriteTool` and applies a `WikiPolicy` (`Allowed` / `Deferred` / `Blocked`). With `Deferred`, every `wiki_write` call is queued instead of executed, and the user reviews the queue with `/wiki review` before any page is committed. Default policy stays `Allowed` so existing behavior is unchanged.

**Batch v2 — Flow Engine** (built on v1):
When the user enters a slash command (`/flow start <template>`, `/brainstorm`, etc.), a new `FlowOrchestrator` drives a YAML-defined sequence of phases through the existing kernel. Plain text input continues to use `AgentSession` unchanged. Each phase has its own prompt, allowed-tool subset, optional iteration cap, and optional review gate. The orchestrator sets `WikiPolicy::Deferred` (from v1) for the duration of the flow, then prompts the user for write approval at flow end.

The default flow is `brainstorm → design → plan → execute → verify`. Templates live in `~/.gasket/flows/` (user) and `gasket/engine/flows/` (built-in fallback). Phase-boundary snapshots in a new `flow_runs` SQLite table enable `/flow resume`. The snapshot stores a copy of the template YAML so resume is reproducible even if the user edits the file mid-flow.

### Key Differences vs. Superseded Design

| Dimension | Old (`phased-agent-loop`) | New (this spec) |
|---|---|---|
| Entry | Config flag, every user message goes phased | Slash command only (v2) — plain text bypasses |
| Phases | Hard-coded 4 (Research/Planning/Execute/Review) | YAML-defined, default 5 (Brainstorm/Design/Plan/Execute/Verify) |
| Customization | None | Per-phase prompt, tools, max_iter, gate; multi-template |
| Wiki write reduction | LLM-driven `wiki_write` in Review phase | **v1**: deferred queue + `/wiki review`; **v2**: orchestrator auto-enables Deferred during flows |
| Persistence | None | `flow_runs` table with `template_yaml` snapshot |
| Gate | None | Explicit per-phase user confirm with `edit/redo/back` |
| Versioning | Single drop | Two batches, v1 ships independently |

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

**v1 batch:**
1. `gasket/engine/src/flow/wiki_guard.rs` — new file (single struct + policy enum)
2. `gasket/engine/src/tools/wiki_tools.rs::WikiWriteTool` — hold `Arc<WikiGuard>` (no dyn); default `WikiPolicy::Allowed` keeps existing behavior
3. `gasket/cli/src/commands/wiki.rs` — extend with `/wiki review` subcommand

**v2 batch (additionally):**
4. `gasket/engine/src/lib.rs` — add `pub mod command;` and `pub mod flow;`
5. CLI entry (`gasket/cli/src/commands/agent.rs`) and gateway entry — route raw user input through `CommandDispatcher::route()` first
6. `gasket/storage/src/` — add `flow_run_store.rs` and a `migrations/flow_run.rs` with the `template_yaml` column
7. `gasket/types/src/events/stream.rs` — add `FlowStarted`, `PhaseChanged`, `GatePending`, `FlowFinished` `ChatEvent` variants

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

Linus simplifications applied: every special-case sentinel is replaced with absence-as-meaning.

```yaml
# ~/.gasket/flows/new-feature.yaml
name: new-feature
description: Full new-feature workflow with design + plan review gates
version: 1

# Global wiki policy for this flow. Defaults to `deferred` — orchestrator
# enables WikiGuard's Deferred policy for the flow's lifetime.
wiki_policy: deferred          # deferred | blocked | allowed

phases:
  - id: brainstorm
    label: "Brainstorm"
    prompt_file: prompts/brainstorm.md      # path relative to template file's dir
    # `allowed_tools` absent → all tools allowed.
    # Present → whitelist only the listed tools.
    allowed_tools: [wiki_search, wiki_read, history_search]
    max_iterations: 5                       # absent → unlimited
    gate:                                   # absent → no gate, auto-advance
      prompt: "Brainstorm direction looks right? (y/n/edit/redo/back)"

  - id: design
    label: "Design"
    prompt_file: prompts/design.md
    allowed_tools: [wiki_search, wiki_read, file_read, file_search]
    max_iterations: 8
    gate:
      prompt: "Design accepted?"

  - id: plan
    label: "Plan"
    prompt_file: prompts/plan.md
    allowed_tools: [wiki_search, file_read, file_search]
    max_iterations: 5
    gate:
      prompt: "Plan looks executable?"

  - id: execute
    label: "Execute"
    prompt_file: prompts/execute.md
    # `allowed_tools` omitted → full tool set
    # `max_iterations` omitted → unlimited
    # `gate` omitted → auto-advance to verify

  - id: verify
    label: "Verify"
    prompt_file: prompts/verify.md
    allowed_tools: [shell, file_read]
    max_iterations: 5
    gate:
      prompt: "Verification passed? End flow?"
```

Removed from prior draft:
- `gate.required: bool` — gate now `Option<GateConfig>`; absence = no gate
- `max_iterations: 0` magic value — now `Option<u32>`; absence = unlimited
- `allowed_tools: ["*"]` magic value — now `Option<Vec<String>>`; absence = all tools

Phase ids are plain strings. Built-in template ids are `brainstorm` / `design` / `plan` / `execute` / `verify`; users can name custom phases anything. There is no special enum distinguishing built-in vs custom — code that needs it calls a small `is_builtin_phase(&str) -> bool` helper.

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
    pub current_phase: PhaseId,           // single source of truth for "where are we"
    pub status: FlowStatus,               // does NOT carry phase
    pub completed_phases: BTreeMap<PhaseId, PhaseOutput>,
    pub pending_wiki_writes: Vec<PendingWikiWrite>,
    /// Set when the user picks `edit <text>` at a gate; consumed and cleared
    /// by the next PhaseRunner invocation.
    pub edit_feedback: Option<String>,
    pub session_key: SessionKey,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Phase identifier — just a string. No enum-vs-custom distinction.
/// Built-in phase ids: "brainstorm", "design", "plan", "execute", "verify".
/// User templates can use any string.
pub type PhaseId = String;

pub fn is_builtin_phase(s: &str) -> bool {
    matches!(s, "brainstorm" | "design" | "plan" | "execute" | "verify")
}

/// Status of a single flow run. Phase information lives in `FlowState.current_phase`.
pub enum FlowStatus {
    Running,
    AwaitingGate,        // no phase field — read FlowState.current_phase
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

Linus simplifications applied:
- **`PhaseId = String`** instead of `enum { Builtin(BuiltinPhase) | Custom(String) }`. The enum had no behavioral differentiation; serialization went through string anyway.
- **`FlowStatus::AwaitingGate` carries no `phase` field**. Phase is read from `FlowState.current_phase`. Eliminates the prior bug where `Back` had to update two fields with the same value.
- **`edit_feedback: Option<String>`** makes `GateResponse::Edit(text)` actually work. The orchestrator sets this on `Edit`; `PhaseRunner` injects it as a user message on the next phase invocation and clears it.

### 4.3 SQLite Schema

A new `flow_run` migration is added under `gasket/storage/src/migrations/` (Rust module, mirroring existing `cron.rs` / `kv.rs` / `session.rs`):

```sql
-- DDL emitted by gasket/storage/src/migrations/flow_run.rs
CREATE TABLE IF NOT EXISTS flow_runs (
  flow_id            TEXT PRIMARY KEY,         -- UUID v7
  template_name      TEXT NOT NULL,
  template_version   INTEGER NOT NULL,
  template_yaml      TEXT NOT NULL,            -- snapshot of YAML at flow start
  user_request       TEXT NOT NULL,
  session_key        TEXT NOT NULL,
  status             TEXT NOT NULL,            -- running|awaiting_gate|paused|done|aborted
  current_phase      TEXT NOT NULL,
  completed_phases   TEXT NOT NULL DEFAULT '{}', -- JSON: {phase_id: PhaseOutput}
  pending_wiki       TEXT NOT NULL DEFAULT '[]', -- JSON: [PendingWikiWrite]
  edit_feedback      TEXT,                     -- nullable; pending edit feedback from a gate
  created_at         INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL
);

CREATE INDEX idx_flow_runs_session ON flow_runs(session_key, updated_at DESC);
CREATE INDEX idx_flow_runs_status ON flow_runs(status, updated_at DESC);
```

The `template_yaml` column stores the exact YAML text that was on disk when the flow was started. Resume uses this snapshot by default — the user editing the file mid-flow does not retroactively change a running flow's behavior. A `GASKET_FLOWS_FORCE_RELOAD=1` environment variable opts into the alternative behavior (reload from disk on resume).

### 4.4 State Machine Transitions

`AwaitingGate` no longer carries a `phase` field. The phase being gated is always `FlowState.current_phase`.

| From `status` | Trigger | To `status` | Side effects |
|---|---|---|---|
| `Running` | PhaseRunner finished, current phase has a `gate` | `AwaitingGate` | record `PhaseOutput`; phase unchanged |
| `Running` | PhaseRunner finished, no gate, more phases follow | `Running` | record `PhaseOutput`; advance `current_phase` |
| `Running` | PhaseRunner finished, no gate, last phase | `Done` | record `PhaseOutput` |
| `Running` | PhaseRunner errored | `Paused` | error logged; flow recoverable via `/flow resume` |
| `AwaitingGate` | user input `y` | `Running` (or `Done` if last) | advance `current_phase` if more |
| `AwaitingGate` | user input `n` | `Aborted` | — |
| `AwaitingGate` | user input `edit <text>` | `Running` | set `edit_feedback = Some(text)`; phase unchanged |
| `AwaitingGate` | user input `redo` | `Running` | clear `completed_phases[current_phase]`; phase unchanged |
| `AwaitingGate` | user input `back` | `AwaitingGate` | set `current_phase = phases[idx-1]` (no-op if idx=0) |
| `Paused` | `/flow resume <id>` | restore prior status | re-issue PhaseRunner from `current_phase` start |
| any non-`Done` | `/flow abort` | `Aborted` | — |

Each transition updates one piece of state at a time. `Back` only changes `current_phase`; status stays `AwaitingGate`. `Edit` only sets `edit_feedback`; the next `PhaseRunner::run` consumes it (prepends a `ChatMessage::user(text)` to the messages list) and clears it.

### 4.5 Resume Behavior

`/flow resume <flow_id>`:
1. `SELECT * FROM flow_runs WHERE flow_id = ?`
2. Reconstruct `FlowState` from `template_yaml` (the snapshot taken at flow start). The on-disk file may have changed; the snapshot is authoritative.
3. If `GASKET_FLOWS_FORCE_RELOAD=1` and the on-disk file's `template_version` differs from the snapshot's, warn the user and reload from disk.
4. If `status = AwaitingGate` → re-print phase summary + gate prompt
5. If `status = Paused` and `current_phase = X` → re-run phase X from scratch (no step-level snapshot — phase-boundary granularity by design)

### 4.6 WikiGuard

Linus simplification: one struct, one `match`, no trait. Replaces the prior trio of `AllowAllGuard` / `BlockingGuard` / `DeferringGuard`.

```rust
// engine/src/flow/wiki_guard.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WikiPolicy { Allowed, Deferred, Blocked }

pub struct WikiGuard {
    policy: WikiPolicy,
    pending: Mutex<Vec<PendingWikiWrite>>,
}

pub enum GuardDecision {
    Allow,                    // pass through to PageStore::write
    Defer,                    // queued; tool returns "deferred for review"
    Reject(String),           // tool returns this string to LLM as error
}

impl WikiGuard {
    pub fn new(policy: WikiPolicy) -> Self {
        Self { policy, pending: Mutex::new(Vec::new()) }
    }

    pub async fn intercept(&self, args: WriteArgsView) -> GuardDecision {
        match self.policy {
            WikiPolicy::Allowed => GuardDecision::Allow,
            WikiPolicy::Blocked => GuardDecision::Reject("blocked".into()),
            WikiPolicy::Deferred => {
                self.pending.lock().unwrap().push(args.into());
                GuardDecision::Defer
            }
        }
    }

    pub fn drain_pending(&self) -> Vec<PendingWikiWrite> {
        std::mem::take(&mut self.pending.lock().unwrap())
    }
}
```

`WikiWriteTool` holds `Arc<WikiGuard>` (no `dyn`, no trait dispatch). Default constructor uses `WikiPolicy::Allowed` so existing call-sites are unchanged.

Usage:
- **v1** — A new `/wiki review` command toggles `policy` to `Deferred`, lets queued writes accumulate, and prompts the user to approve/discard.
- **v2** — `FlowOrchestrator::start_new` reads `wiki_policy` from the template, builds a fresh `WikiGuard`, and swaps it into `WikiWriteTool` for the flow's lifetime. On `Done` it drains the queue and prompts the user.

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
| Unit | `WikiGuard` | `flow/wiki_guard_test.rs` | All three `WikiPolicy` branches (`Allowed`/`Deferred`/`Blocked`); drain queue idempotency |
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

Two independent batches. Batch v1 ships first, gets ~1 week of real use, then batch v2 starts. Each batch produces a working, testable system on its own.

### Batch v1 — Wiki Write Guard (4 tasks)

1. **`gasket/engine/src/flow/wiki_guard.rs`** — single `WikiGuard` struct + `WikiPolicy` enum + `GuardDecision` enum
2. **`gasket/engine/src/tools/wiki_tools.rs`** — `WikiWriteTool::with_policy(policy)` builder; default `Allowed` keeps existing behavior
3. **`gasket/cli/src/commands/wiki.rs`** — `/wiki review` subcommand: toggle to Deferred, list queue, approve/discard interactively
4. **End-to-end smoke** — confirm Allowed still writes immediately; Deferred queues; review approves

### Batch v2 — Flow Engine (4 tasks, depends on v1's `WikiGuard`)

5. **`gasket/types/src/flow.rs`** — `PhaseId = String`, `FlowStatus` (no phase field), `PhaseOutput`, `PendingWikiWrite`; **`gasket/engine/src/flow/state.rs`** — `FlowState` with `edit_feedback`, pure state-machine transitions
6. **`gasket/storage/src/migrations/flow_run.rs`** + **`gasket/storage/src/flow_run_store.rs`** — DDL with `template_yaml` column + CRUD
7. **`gasket/engine/src/flow/{template,phase_runner,gate,orchestrator}.rs`** — YAML loader; `PhaseRunner` that **really calls `kernel::execute_streaming`** (no stub); `CliGate` for stdin; `FlowOrchestrator` ties them together
8. **`gasket/engine/src/command/{parser,dispatcher}.rs`** + **`gasket/cli/src/commands/agent.rs`** — slash-command parser + dispatcher + CLI wiring (real, not stub); built-in templates ship with the binary

After v2 lands: web frontend gate UI is a follow-up plan (not gating v2 ship).

---

## 8. Out of Scope (v1 + v2)

- Conditional phase branching ("if execute fails go back to plan"). Templates are linear.
- Step-level resume (only phase-boundary).
- Concurrent flows in a single session (one active flow per session).
- Multi-user gate approval.
- Web UI gate buttons polish — events emitted, full UI is a follow-up.
- Editing a running flow's template hot. Snapshot in `template_yaml` column makes this safe-by-default; opt-in reload via `GASKET_FLOWS_FORCE_RELOAD=1`.

**Not in this list (intentionally):** Per-phase kernel invocation with `allowed_tools` filter is **mandatory** in batch v2 — the orchestrator's `PhaseRunner` calls `kernel::execute_streaming` for real, with the phase's tool-filtered registry. Earlier draft deferred this to v1.1; the Linus review correctly flagged that as shipping vapor. Batch v2 has no such carve-out.
