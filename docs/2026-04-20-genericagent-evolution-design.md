# GenericAgent-Inspired Evolution Architecture for Gasket

## Status

**Draft** — Pending spec review and user approval.

## Summary

This spec proposes integrating GenericAgent's core innovations — self-evolving skills, structured plan/verify execution, hierarchical memory indexing, and monitored subagent delegation — into gasket's Rust architecture. The design follows a "thin orchestrator" philosophy: a `PlanExecutor` state machine orchestrates complex tasks via a new `SteppableExecutor` primitive, while the existing Direct execution path (via `KernelExecutor`) remains untouched.

Five modules are defined:

1. **InsightIndex** — Wiki navigation layer (L1-equivalent) for O(1) topic-to-SOP routing
2. **PlanExecutor** — Optional 4-phase state machine (Explore/Plan/Execute/Verify) for complex tasks
3. **MonitoredSpawner** — Real-time monitoring and intervention for subagents via channels + SQLite fallback
4. **EvolutionHook Enhancement** — Task-to-SOP crystallization with automatic InsightIndex updates
5. **Compactor Checkpoint** — Proactive working-memory snapshots every N turns

## Key Design Decision: Steppable Execution Primitive

The spec reviewer identified that `KernelExecutor` runs a **full autonomous loop** (`for iteration in 1..=max_iterations`) with no public API for per-step external orchestration. To enable PlanExecutor's step-by-step execution without rewriting the kernel, we introduce a new **`SteppableExecutor`** primitive:

- `SteppableExecutor` is a **refactored extraction** from `KernelExecutor` that splits the loop body into discrete `step()` calls
- `KernelExecutor` internally uses `SteppableExecutor` (composition, not inheritance), preserving its existing API
- PlanExecutor calls `SteppableExecutor::step()` for each plan step, controlling iteration count and message history externally
- The streaming pipeline (`StreamEvent`) works identically — events flow per-step and aggregate

This is a **surgical refactor**: extract the loop body into `step()`, have `KernelExecutor` call `step()` in a loop. No behavior change for existing callers.

## Background

### GenericAgent's Key Innovations

GenericAgent (`lsdefine/GenericAgent`) is a ~3K-line Python framework proving that a minimal agent core can achieve powerful self-evolution through:

- **9 atomic tools** + ~100-line agent loop → full system control
- **L0-L4 hierarchical memory** — L1 Insight Index (≤30 lines) is the critical navigation layer
- **Plan SOP mode** — explicit `[D]`/`[P]`/`[?]`/`[VERIFY]` markers turn implicit reasoning into enforceable protocol
- **Subagent file-IO protocol** — `_intervene`, `_keyinfo`, `_stop` files enable real-time oversight
- **"No Execution, No Memory" axiom** — prevents hallucination from polluting long-term memory

### Gasket's Current State

Gasket has a solid Rust foundation:

- **KernelExecutor** (`kernel/executor.rs`) — structured LLM loop with `ExecutionState` + `TokenLedger`
- **Wiki knowledge system** (`wiki/`) — SQLite + Tantivy three-layer architecture
- **Hooks pipeline** (`hooks/`) — `BeforeRequest`/`AfterHistory`/`BeforeLLM`/`AfterToolCall`/`AfterResponse`
- **EvolutionHook** — already extracts memories from conversations into wiki pages
- **ContextCompactor** — watermark-based background summarization
- **Subagent spawner** — functional API (`spawn_subagent`, `SimpleSpawner`)
- **ToolRegistry** — embedding-based Top-K tool routing
- **SkillsRegistry** — Markdown skill loading with dependency checking

### The Gap

Gasket has the pieces but lacks the **protocol** tying them together:

- No structured plan/verify execution for complex tasks
- No real-time subagent monitoring
- Wiki has search (Tantivy) but no **navigation index** for LLM self-routing
- EvolutionHook extracts facts but not **action-verified procedural skills (SOPs)**
- ContextCompactor is passive — no proactive working-memory checkpoints

## Goals

1. Enable gasket to handle complex multi-step tasks with structured planning and independent verification
2. Let the agent **discover and reuse its own skills** via an auto-maintained SOP index
3. Provide real-time oversight of subagent execution with intervention capabilities
4. Ensure task learnings are **automatically crystallized** into reusable SOPs
5. Maintain proactive working memory without heuristic turn-based patches

## Non-Goals

- Replacing the existing Direct execution mode — PlanExecutor is strictly optional
- Porting GenericAgent's Python implementation verbatim — we adapt concepts to Rust idioms
- Supporting GenericAgent's browser automation (webdriver) — out of scope
- Changing the Actor pipeline architecture (Router→Session→Outbound)

## Changes from Review (Iteration 1)

The following critical issues were identified and fixed during the first spec review:

1. **KernelExecutor not steppable** — Spec originally assumed per-step calls, but `KernelExecutor.run_loop()` runs a full autonomous loop. **Fix**: Introduced `SteppableExecutor` as a new primitive — extracted loop body into `step()` method, with `KernelExecutor` composing it internally. Zero API change for existing callers.

2. **AgentSession.process_direct() can't be branched** — No hook point existed for `ComplexityAssessor` before kernel runs. **Fix**: Added new `AgentSession.process_with_plan()` method plus `process_auto()` convenience wrapper. Caller chooses between direct and plan mode.

3. **SQLite FK to non-existent sessions table** — `execution_plans` had `FOREIGN KEY REFERENCES sessions(key)` but no `sessions` table exists. **Fix**: Removed FK, added `idx_execution_plans_session` index.

4. **MonitoredSpawner can't decorate runner.rs** — `run_subagent()` is a pure function with no internal hooks. **Fix**: `MonitoredRunner` implements its own execution loop using `SteppableExecutor`, not as a decorator around `run_subagent()`.

5. **Missing sops/ directory** — `PageStore::init_dirs()` doesn't create "sops". **Fix**: Added to Migration Plan Phase 1.

**Warnings also fixed**:
- `std::sync::RwLock` → `tokio::sync::RwLock`
- Overloaded `marker` field split into `step_type` + `status`
- `page_store.read()` dedup → `page_store.exists()`
- Static `assess()` method → instance method with provider/model
- Checkpoint injection moved from direct state mutation to caller-layer (`BeforeLLM` was considered but rejected because it fires once per request, not per iteration)

## Changes from Review (Iteration 2)

The following issues were identified and fixed during the second spec review:

1. **Checkpoint injection at wrong layer** — `BeforeLLM` hook fires once per request in `ContextBuilder::build()`, not per kernel iteration. `KernelExecutor` has zero hook infrastructure. **Fix**: Checkpoint injection moved to the **caller layer** (`PlanExecutor` calls `compactor.checkpoint()` between `step()` calls). Direct mode (`KernelExecutor`) keeps only passive compaction.

2. **AgentSession missing `insight_index` field** — `process_with_plan()` referenced `self.insight_index` but `AgentSession` has no such field. **Fix**: Added explicit note that `AgentSession` gains `insight_index: Option<Arc<InsightIndex>>` in Phase 1 of the migration plan.

3. **EvolutionHook missing `insight_index` field** — `persist_as_sop()` used `self.insight_index` but the struct had no such field. **Fix**: Added note that `EvolutionHook` gains `insight_index` field and `with_insight_index()` builder method.

4. **MonitoredSpawner::spawn() borrow checker error** — Captured `self` in `tokio::spawn` after calling `&self` methods. **Fix**: Clone `sqlite_pool` before spawn; pass cloned pool into async block.

5. **ComplexityAssessor contradictory definitions** — Had both `pub struct ComplexityAssessor;` and `pub struct ComplexityAssessor { provider, model }`. **Fix**: Removed unit struct, kept the one with fields.

6. **PageType::Sop missing `as_str()` and `FromStr`** — Added `Sop` variant but didn't show `as_str()` → `"sop"` or `FromStr` parsing. **Fix**: Added both implementations in the spec.

7. **`fix` step_type premature** — Schema included `fix` as a step type but Open Question #2 was unresolved. **Fix**: Removed `fix` from `step_type` enum; fix-loop behavior will be handled by appending new steps with `status = 'pending'`.

8. **MonitoredRunner missing tools/provider fields** — `run()` called `self.llm.chat()` and `self.execute_tool()` but struct had no such fields. **Fix**: `MonitoredRunner` now uses `SteppableExecutor` internally (which owns provider + tools + config).

9. **SteppableExecutor missing `TokenLedger`** — `step()` returned `token_usage: Option<TokenUsage>` but caller had no way to accumulate across steps. **Fix**: Added `ledger: &mut TokenLedger` parameter to `step()`. KernelExecutor creates ledger internally and passes it each iteration.

## Architecture

### Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        AgentSession                           │
│  ┌──────────────────────┐    ┌──────────────────────────┐  │
│  │   process_direct()   │───►│   ComplexityAssessor     │  │
│  └──────────────────────┘    │   (1-round LLM judgment)  │  │
│                              └───────────┬──────────────┘  │
│                                          │                 │
│                    simple (<3 steps)     │   complex (≥3)  │
│                                          ▼                 │
│                              ┌──────────────────────────┐  │
│                              │     PlanExecutor         │  │
│                              │  ┌────┬────┬────┬────┐   │  │
│                              │  │Ex │Pl │Ex │Ve │   │  │
│                              │  │pl │an │ec │ri │   │  │
│                              │  │or │  │ut │fy │   │  │
│                              │  └────┴────┴────┴────┘   │  │
│                              └──────────────────────────┘  │
│                                          │                 │
│                              ┌───────────┴───────────┐    │
│                              ▼                       ▼    │
│                    ┌──────────────┐        ┌─────────────┐ │
│                    │ Steppable    │        │ Subagent    │ │
│                    │ Executor     │        │ (Monitored) │ │
│                    └──────────────┘        └─────────────┘ │
│                              │                       │    │
│                              ▼                       ▼    │
│                    ┌──────────────┐        ┌─────────────┐ │
│                    │ InsightIndex │        │ EvolutionHook│ │
│                    │   (wiki)     │        │  (hooks)     │ │
│                    └──────────────┘        └─────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**Key Interface Contracts:**

1. `PlanExecutor` → `SteppableExecutor`: Per-step execution with external iteration control
2. `PlanExecutor` → `MonitoredSpawner`: `spawn_monitored()` returns `(handle, interventor, progress_rx)`
3. `PlanExecutor` → `InsightIndex`: `insight.lookup(query, k) -> Vec<InsightEntry>` for SOP routing
4. `EvolutionHook` → `InsightIndex`: Atomic upsert after SOP creation

### Module Dependency Graph

```
PlanExecutor
    ├── uses MonitoredSpawner (Explore, Verify phases)
    ├── uses InsightIndex (Plan phase — SOP lookup)
    ├── uses SteppableExecutor (Execute phase — per-step)
    └── writes ExecutionPlan + ExecutionStep tables

MonitoredSpawner
    ├── wraps SimpleSpawner
    ├── uses SQLite fallback table
    └── injects progress/intervention into runner

InsightIndex
    ├── uses SQLite (wiki_insights table)
    ├── references PageStore (wiki pages)
    └── synced by EvolutionHook + background task

EvolutionHook (enhanced)
    ├── uses PageStore (write SOP pages)
    ├── uses InsightIndex (index new SOPs)
    └── triggered at AfterResponse hook point

Compactor (enhanced)
    ├── adds checkpoint() method
    ├── writes session_checkpoints table
    └── called from executor loop every N turns
```

## Detailed Design

### 1. InsightIndex Module

#### Purpose

A navigation layer sitting above the Wiki system. It does not store content — only "topic → page_path" pointers with relevance scoring. This is the L1-equivalent from GenericAgent's memory hierarchy.

#### Data Structures

```rust
#[derive(Debug, Clone)]
pub struct InsightEntry {
    pub topic: String,              // Trigger keyword, e.g. "docker_build"
    pub page_path: String,          // Wiki page path, e.g. "sops/docker_build"
    pub page_type: PageType,        // Entity | Topic | Source | Sop
    pub relevance: f32,             // 0.0-1.0, sort tiebreaker
    pub last_verified: Option<DateTime<Utc>>,
}

pub struct InsightIndex {
    pool: SqlitePool,
    cache: tokio::sync::RwLock<HashMap<String, Vec<InsightEntry>>>,
}
```

#### API

```rust
impl InsightIndex {
    /// O(1) routing query used by PlanExecutor during planning.
    pub async fn lookup(&self, query: &str, k: usize) -> Vec<InsightEntry>;

    /// Atomic upsert called by EvolutionHook after SOP creation.
    pub async fn upsert(&self, entry: &InsightEntry) -> Result<()>;

    /// Background sync: scan wiki for new Sop pages and auto-index.
    pub async fn sync_with_wiki(&self, store: &PageStore) -> Result<SyncReport>;
}
```

#### SQLite Schema

```sql
CREATE TABLE wiki_insights (
    topic TEXT NOT NULL,
    page_path TEXT NOT NULL PRIMARY KEY,
    page_type TEXT NOT NULL,
    relevance REAL DEFAULT 1.0,
    last_verified TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_insight_topic ON wiki_insights(topic);
CREATE INDEX idx_insight_type ON wiki_insights(page_type);
```

#### PageType::Sop Addition

```rust
pub enum PageType {
    Entity,
    Topic,
    Source,
    Sop,  // NEW
}

impl PageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Topic => "topic",
            Self::Source => "source",
            Self::Sop => "sop",  // NEW
        }
    }

    pub fn directory(&self) -> &'static str {
        match self {
            Self::Entity => "entities",
            Self::Topic => "topics",
            Self::Source => "sources",
            Self::Sop => "sops",
        }
    }
}

impl std::str::FromStr for PageType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "entity" => Ok(Self::Entity),
            "topic" => Ok(Self::Topic),
            "source" => Ok(Self::Source),
            "sop" => Ok(Self::Sop),  // NEW
            _ => Err(()),
        }
    }
}
```

#### SOP Page Template

EvolutionHook generates SOP pages with this frontmatter + body structure:

```markdown
---
title: Docker Build SOP
type: sop
tags: [auto_learned, docker]
---

## Trigger Scenario
- User requests building a Docker image

## Preconditions
- Docker daemon running
- Dockerfile exists in working directory

## Key Steps
1. [ ] Check Dockerfile exists
2. [ ] Run `docker build`
3. [ ] Verify image created

## Pitfalls
- Do not pull external base images in isolated network environments
```

#### Integration Points

- **PlanExecutor** calls `lookup()` to find relevant SOPs before planning
- **EvolutionHook** calls `upsert()` atomically after creating a new SOP page
- **Background task** runs `sync_with_wiki()` periodically to catch orphaned pages

---

### 1b. SteppableExecutor (New Primitive)

#### Purpose

Extract the per-iteration body of `KernelExecutor.run_loop()` into a reusable `SteppableExecutor` that can be driven externally. `KernelExecutor` internally composes `SteppableExecutor` in a loop, preserving its existing API.

#### API

```rust
/// Result of executing one LLM iteration
pub struct StepResult {
    pub response: ChatResponse,
    pub tool_results: Vec<ToolCallResult>,
    pub token_usage: Option<TokenUsage>,
    pub should_continue: bool,  // true if tool_calls present
}

/// Steppable executor — one LLM call + optional tool execution per step()
pub struct SteppableExecutor {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: KernelConfig,
}

impl SteppableExecutor {
    /// Execute one iteration: LLM call → optional tool calls → return result
    pub async fn step(
        &self,
        messages: &mut Vec<ChatMessage>,
        ledger: &mut TokenLedger,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Result<StepResult, KernelError>;
}
```

#### KernelExecutor Integration

```rust
// KernelExecutor::run_loop() becomes:
pub async fn run_loop(...) -> Result<ExecutionResult, KernelError> {
    let steppable = SteppableExecutor::new(provider, tools, config);
    let mut ledger = TokenLedger::new();
    for iteration in 1..=config.max_iterations {
        let result = steppable.step(&mut state.messages, &mut ledger, event_tx).await?;
        if !result.should_continue {
            return Ok(state.to_result(result.response, &ledger));
        }
    }
    Err(KernelError::MaxIterations(config.max_iterations))
}
```

**Zero behavior change for existing callers.** `KernelExecutor` API is unchanged; only its internal implementation uses `SteppableExecutor`. The `TokenLedger` is created inside `run_loop` and accumulated across `step()` calls, exactly as the original inline loop did.

---

### 2. PlanExecutor Module

#### Purpose

A finite state machine executing complex tasks through 4 phases: Explore → Plan → Execute → Verify. It wraps `KernelExecutor` as a per-step caller, leaving the core loop untouched.

#### State Machine

```
Created ──assess()──► Direct (simple tasks, skip PlanExecutor)
    │
    └──► Exploring ──findings ready──► Planning ──user confirm──►
                                                          Executing ──all [✓]──► Verifying
                                                                │                      │
                                                                │ step fail            ├── PASS ──► Done
                                                                │                      │
                                                                └── [FIX] loop ◄── FAIL ──┘
```

#### ExecutionPlan Data Structures

```sql
CREATE TABLE execution_plans (
    plan_id TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    task_description TEXT NOT NULL,
    status TEXT NOT NULL,  -- Created | Exploring | Planning | Executing | Verifying | Done | Aborted
    findings TEXT,         -- JSON: ExplorationResult
    plan_wiki_path TEXT,   -- Reference to plan.md in wiki
    current_step INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMP,
    max_retries INTEGER DEFAULT 3,
    verification_result TEXT  -- PASS | FAIL | PARTIAL
);

CREATE INDEX idx_execution_plans_session ON execution_plans(session_key);

CREATE TABLE execution_steps (
    step_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    step_number INTEGER NOT NULL,
    description TEXT NOT NULL,
    step_type TEXT NOT NULL DEFAULT 'direct',  -- direct | delegated | parallel | conditional
    status TEXT NOT NULL DEFAULT 'pending',    -- pending | done | failed | skipped
    sop_path TEXT,
    depends_on INTEGER,
    condition TEXT,        -- For [?] conditional branches
    tool_calls TEXT,       -- JSON: [ToolCallRecord]
    result_summary TEXT,
    error TEXT,
    FOREIGN KEY (plan_id) REFERENCES execution_plans(plan_id)
);
```

#### ComplexityAssessor (Entry Point)

```rust
pub enum Complexity {
    Direct,  // 1-2 steps, skip PlanExecutor
    Auto,    // 3 steps, let agent decide
    Plan,    // 4+ steps or dependencies, force Plan mode
}

pub struct ComplexityAssessor {
    provider: Arc<dyn LlmProvider>,
    model: String,  // Cheap model, e.g. "anthropic/claude-haiku"
}

impl ComplexityAssessor {
    /// 1-round judgment, ≤100 tokens
    pub async fn assess(&self, task: &str) -> Complexity;
}
```

#### Phase Details

**Explore Phase:**
- Spawn a MonitoredSpawner subagent with an exploration task
- Subagent writes findings to a wiki exploration page
- PlanExecutor monitors progress via `progress_rx`, intervenes via `interventor`
- Subagent constrained to read-only operations, ≤10 tool calls

**Plan Phase:**
- Query InsightIndex for relevant SOPs
- Build prompt: findings + SOP references + plan generation instructions
- LLM generates structured plan with `step_type` markers (`delegated`/`parallel`/`conditional`) and a final `verify` step
- Write plan to wiki + SQLite execution_steps table
- `ask_user` for confirmation before execution

**Execute Phase:**
- For each step in order:
  - Check dependencies satisfied; if not, mark `status = skipped`
  - `step_type = delegated` → spawn MonitoredSpawner subagent
  - `step_type = parallel` → collect parallel subagents, await all
  - `step_type = direct` → call SteppableExecutor for single-step execution
  - Mini-verify: quick sanity check of output
  - Mark `status = done` and persist to SQLite

**Verify Phase:**
- Spawn independent verification subagent with adversarial role
- Subagent reads plan + deliverables, runs verification checks
- Subagent outputs `VERDICT: PASS | FAIL | PARTIAL` as final line
- **PASS** → mark verify step `status = done`, call EvolutionHook
- **FAIL** → enter fix loop (max `max_retries` iterations)
- **PARTIAL** → ask user to decide

#### Error Handling

- **Subagent failure** → check stderr log, retry up to 2 times, then escalate to user
- **Step failure** → mark `[✗]`, record error, retry 3× with backoff (2s/4s/8s)
- **Dependency failure** → propagate `[SKIP]` to downstream steps
- **Verify FAIL** → append `[FIX]` steps to plan, re-execute only failed items

---

### 3. MonitoredSpawner Module

#### Purpose

Add real-time monitoring and intervention to subagent execution. Replaces GenericAgent's file-IO protocol (`_intervene`, `_stop`, `_keyinfo`) with type-safe Rust channels backed by SQLite fallback.

#### Architecture

```
Main Agent                              Subagent
  │                                       │
  │  spawn_monitored(spec)                │
  ├──────────────────────────────────────►│
  │  ◄── (handle, interventor, progress_rx)│
  │                                       │
  │  ◄────── ProgressUpdate ──────────────┤
  │  ────── Intervention ────────────────►│
  │                                       │
  │  ◄────── SubagentResult ──────────────┤
```

#### Types

```rust
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    Thinking { turn: usize },
    ToolStart { name: String, args: String },
    ToolResult { name: String, output: String, duration_ms: u64 },
    TurnComplete { turn: usize, summary: String },
    Done { result: String },
    Error { message: String },
}

#[derive(Debug, Clone)]
pub enum Intervention {
    Abort,
    AddKeyInfo(String),
    AppendPrompt(String),
    ExtendTurns(u32),
}

pub struct MonitoredHandle {
    pub handle: JoinHandle<SubagentResult>,
    pub interventor: mpsc::Sender<Intervention>,
    pub progress: mpsc::Receiver<ProgressUpdate>,
}
```

#### Communication: Channel + SQLite Fallback

```rust
pub struct MonitoredSpawner {
    inner: SimpleSpawner,
    sqlite_pool: SqlitePool,
}

impl MonitoredSpawner {
    pub async fn spawn(&self, spec: TaskSpec) -> Result<MonitoredHandle> {
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (interventor_tx, interventor_rx) = mpsc::channel(16);

        // Register in DB for crash recovery
        self.register_in_db(&spec.id, &spec).await?;

        // Clone pool for the spawned task (can't capture `self` across await)
        let pool = self.sqlite_pool.clone();
        let spec_id = spec.id.clone();

        let handle = tokio::spawn(async move {
            let mut runner = MonitoredRunner::new(spec, progress_tx, interventor_rx);
            let result = runner.run().await;
            // Update DB via cloned pool
            Self::update_db_status(&pool, &spec_id, &result).await?;
            result
        });

        Ok(MonitoredHandle { handle, interventor: interventor_tx, progress: progress_rx })
    }
}
```

#### MonitoredRunner

Wraps the existing subagent runner, injecting progress/intervention hooks at key points:

```rust
struct MonitoredRunner {
    spec: TaskSpec,
    progress: mpsc::Sender<ProgressUpdate>,
    intervention: mpsc::Receiver<Intervention>,
    steppable: SteppableExecutor,  // Uses provider + tools + config
    messages: Vec<ChatMessage>,
    ledger: TokenLedger,
}

impl MonitoredRunner {
    async fn run(&mut self) -> SubagentResult {
        for turn in 1..=self.spec.max_turns {
            // Check for interventions (non-blocking)
            while let Ok(i) = self.intervention.try_recv() {
                self.apply_intervention(i)?;
            }

            self.progress.send(ProgressUpdate::Thinking { turn }).await.ok();

            // Execute one LLM iteration via SteppableExecutor
            let result = self.steppable.step(&mut self.messages, &mut self.ledger, None).await?;

            if !result.tool_results.is_empty() {
                for tr in &result.tool_results {
                    self.progress
                        .send(ProgressUpdate::ToolStart { name: tr.tool_name.clone() })
                        .await
                        .ok();
                    self.progress
                        .send(ProgressUpdate::ToolResult {
                            name: tr.tool_name.clone(),
                            output: tr.output.clone(),
                        })
                        .await
                        .ok();
                }
            }

            self.progress
                .send(ProgressUpdate::TurnComplete {
                    turn,
                    summary: result.response.content.clone().unwrap_or_default(),
                })
                .await
                .ok();

            if !result.should_continue {
                break;
            }
        }

        let final_content = self.messages.last()
            .and_then(|m| m.content.clone())
            .unwrap_or_default();
        self.progress.send(ProgressUpdate::Done { result: final_content.clone() }).await.ok();
        SubagentResult::Success(final_content)
    }
}
```

#### SQLite Fallback Table

```sql
CREATE TABLE subagent_tasks (
    task_id TEXT PRIMARY KEY,
    spec TEXT NOT NULL,
    status TEXT NOT NULL,
    progress_log TEXT,
    interventions TEXT,
    result TEXT,
    started_at TIMESTAMP,
    completed_at TIMESTAMP
);

CREATE INDEX idx_subagent_status ON subagent_tasks(status);
```

#### Integration with Existing runner.rs

**Important**: The existing `run_subagent()` in `runner.rs` is a **pure function** that creates a `KernelExecutor` and runs it to completion. It has no internal hooks for progress observation or intervention.

Therefore, `MonitoredRunner` is **not a decorator** around `run_subagent()`. Instead:

1. `MonitoredRunner` implements its own execution loop using `SteppableExecutor`
2. Progress events are emitted at each turn boundary
3. Interventions are checked between turns via `try_recv()`
4. The existing `SimpleSpawner` API remains unchanged for non-monitored use cases

Future enhancement: Add `ProgressHook` trait to `KernelExecutor` so that any execution (direct or subagent) can emit progress events uniformly.

---

### 4. EvolutionHook Enhancement

#### Purpose

Extend the existing `EvolutionHook` to classify extracted memories by type, write SOPs as `PageType::Sop`, and atomically update the InsightIndex.

**Struct change:** `EvolutionHook` gains `insight_index: Option<Arc<InsightIndex>>` (mirroring the existing `page_store` field) and a `with_insight_index()` builder method. If `insight_index` is `None`, SOPs are still written to the wiki but the InsightIndex is not updated.

#### Memory Classification

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvolutionMemory {
    title: String,
    memory_type: String,   // "note" | "skill"
    scenario: String,      // "profile" | "knowledge" | "procedure"
    content: String,
    tags: Option<Vec<String>>,
    verified: bool,        // Backed by successful tool result
    confidence: f32,       // 0.0-1.0
}
```

#### Classification Logic

```rust
impl EvolutionHook {
    async fn persist_memory(&self, mem: &EvolutionMemory) -> Result<(), AgentError> {
        match mem.memory_type.as_str() {
            "note" => self.persist_as_fact(mem).await,    // L2: Entity/Topic
            "skill" => self.persist_as_sop(mem).await,    // L3: Sop + InsightIndex
            _ => self.persist_as_topic(mem).await,        // Fallback
        }
    }
}
```

#### SOP Persistence Path

```rust
async fn persist_as_sop(&self, mem: &EvolutionMemory) -> Result<(), AgentError> {
    let page_store = self.page_store.as_ref()
        .ok_or_else(|| AgentError::Other("PageStore not available".into()))?;

    // Deduplication
    let slug = slugify(&mem.title);
    let path = format!("sops/{}", slug);
    if page_store.exists(&path).await? {
        return Ok(());  // Already exists
    }

    // Create Sop page
    let mut page = WikiPage::new(
        path.clone(),
        mem.title.clone(),
        PageType::Sop,
        format_sop_content(mem),
    );
    page.tags = mem.tags.clone().unwrap_or_default();
    page.tags.push("auto_learned".to_string());
    if mem.verified {
        page.tags.push("verified".to_string());
    }

    page_store.write(&page).await?;

    // Update InsightIndex atomically
    if let Some(ref insight) = self.insight_index {
        insight.upsert(&InsightEntry {
            topic: mem.title.clone(),
            page_path: path,
            page_type: PageType::Sop,
            relevance: mem.confidence,
            last_verified: if mem.verified { Some(Utc::now()) } else { None },
        }).await?;
    }

    Ok(())
}
```

#### Enhanced Extraction Prompt

The LLM extraction prompt is enhanced to enforce GenericAgent's "No Execution, No Memory" axiom:

```
Analyze the conversation and extract ONLY action-verified memories.

CRITICAL: "No Execution, No Memory" — only include facts confirmed by tool calls.

For each item, classify:
- type: "note" (factual) or "skill" (procedural)
- scenario: "profile" (user pref), "knowledge" (env fact), "procedure" (task skill)
- verified: true if backed by successful tool result
- confidence: 0.0-1.0 based on verification strength

Output: [{"title", "type", "scenario", "content", "tags", "verified", "confidence"}]
```

---

### 5. Compactor Checkpoint

#### Purpose

Add proactive working-memory snapshots every N turns, eliminating the need for heuristic patches like GenericAgent's `turn % 7 == 0` checks.

#### Configuration

```rust
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    pub interval_turns: usize,  // Default: 7
    pub checkpoint_prompt: String,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            interval_turns: 7,
            checkpoint_prompt: r#"Summarize current task state for working memory.
Output ONLY in this format:

<key_info>
- Current goal: [one sentence]
- Completed: [list]
- Blocked on: [if any]
- Next step: [one sentence]
- Key facts learned: [list]
</key_info>

Be concise."#.into(),
        }
    }
}
```

#### Checkpoint Method

```rust
impl ContextCompactor {
    /// Called from executor loop every N turns.
    pub async fn checkpoint(
        &self,
        session_key: &SessionKey,
        current_turn: usize,
        recent_events: &[SessionEvent],
    ) -> Result<Option<String>> {
        if current_turn % self.checkpoint_config.interval_turns != 0 {
            return Ok(None);
        }

        let prompt = format!("{}\n\nRecent events:\n{}",
            self.checkpoint_config.checkpoint_prompt,
            Self::format_events(recent_events)
        );

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system("You are a state summarizer."),
                ChatMessage::user(prompt),
            ],
            max_tokens: Some(512),
            temperature: Some(0.2),
            ..Default::default()
        };

        let response = self.provider.chat(request).await?;
        let summary = response.content.unwrap_or_default();

        self.sqlite_store
            .save_checkpoint(session_key, current_turn, &summary)
            .await?;

        Ok(Some(summary))
    }
}
```

#### SQLite Table

```sql
CREATE TABLE session_checkpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL,
    turn INTEGER NOT NULL,
    summary TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_key, turn)
);
```

#### Integration: Caller-Layer Injection

`KernelExecutor.run_loop()` has **no hook infrastructure** — it is a pure LLM iteration loop. The existing `BeforeLLM` hook fires **once per request** in `ContextBuilder::build()`, before the kernel is even invoked. Therefore, proactive per-iteration checkpoints cannot flow through `BeforeLLM`.

Instead, checkpoint injection happens at the **caller layer**, depending on which executor is used:

**Direct mode (`KernelExecutor`)**: No proactive checkpoint injection. Only existing passive compaction (`try_compact` when token threshold is exceeded) is active. Proactive checkpoints require the steppable executor.

**Plan mode (`SteppableExecutor`)**: `PlanExecutor` calls `compactor.checkpoint()` between `step()` calls and injects the result into the message history:

```rust
// PlanExecutor::run() — inside the execution loop
let mut ledger = TokenLedger::new();
for step in &plan.steps {
    // 1. Checkpoint every N turns
    if turn % checkpoint_config.interval_turns == 0 {
        if let Ok(Some(checkpoint)) = compactor
            .checkpoint(session_key, turn, &recent_events)
            .await
        {
            messages.push(ChatMessage::system(format!(
                "[Checkpoint at turn {}]\n{}",
                turn, checkpoint
            )));
        }
    }

    // 2. Execute the plan step
    let result = steppable.step(&mut messages, &mut ledger, event_tx).await?;
    // ...
}
```

**Future extension**: A new `BeforeIteration` hook point could be added to `KernelExecutor` so that direct mode also benefits from proactive checkpoints. This is out of scope for this spec.

**Checkpoint vs. Summary distinction:**
- **Summary** (existing) = compressed historical conversation, passive, token-driven
- **Checkpoint** (new) = structured working state, proactive, turn-driven

---

## Data Flow

### Task Lifecycle (Complete Flow)

```
User Request
    │
    ▼
AgentSession.process_direct()
    │
    ▼
ComplexityAssessor.assess()
    │
    ├── simple ──► KernelExecutor.execute() ──► Done
    │
    └── complex ──► PlanExecutor.run()
                        │
                        ▼
                    Explore Phase
                        ├── MonitoredSpawner.spawn(explore_spec)
                        ├── Progress monitoring + intervention
                        └── Write findings to wiki exploration page
                        │
                        ▼
                    Plan Phase
                        ├── InsightIndex.lookup(task, 5)
                        ├── LLM generates plan with markers
                        ├── Persist to execution_steps table
                        └── ask_user confirmation
                        │
                        ▼
                    Execute Phase
                        ├── For each step:
                        │   ├── Check dependencies
                        │   ├── Route: delegated→subagent, parallel→parallel, default→SteppableExecutor
                        │   ├── Mini-verify output
                        │   └── Mark status = done in SQLite
                        └── All steps done
                        │
                        ▼
                    Verify Phase
                        ├── MonitoredSpawner.spawn(verify_spec)
                        ├── Adversarial verification
                        ├── Parse VERDICT line
                        └── PASS? → Done / FAIL? → fix loop
                        │
                        ▼
                    Done
                        ├── EvolutionHook.on_task_complete()
                        │   ├── Extract verified facts/skills
                        │   ├── Write SOP to wiki (PageType::Sop)
                        │   └── Upsert InsightIndex
                        └── Archive plan to wiki
```

#### AgentSession Integration

PlanExecutor does **not** modify `process_direct()`. Instead, a new method `process_with_plan()` is added:

```rust
impl AgentSession {
    /// Process a message with plan-mode support.
    /// Called when ComplexityAssessor indicates a complex task.
    pub async fn process_with_plan(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        let plan_executor = PlanExecutor::new(
            self.runtime_ctx.clone(),
            self.insight_index.clone(),
            self.page_store.clone(),
        );
        plan_executor.run(content, session_key).await
    }
}
```

The caller (CLI or channel handler) is responsible for choosing between `process_direct()` and `process_with_plan()`. A convenience wrapper can automate this:

```rust
pub async fn process_auto(
    &self,
    content: &str,
    session_key: &SessionKey,
) -> Result<AgentResponse, AgentError> {
    let assessor = ComplexityAssessor::new(self.provider.clone(), "cheap-model".into());
    match assessor.assess(content).await? {
        Complexity::Direct => self.process_direct(content, session_key).await,
        Complexity::Plan => self.process_with_plan(content, session_key).await,
        Complexity::Auto => {
            // Let the agent decide via a tool call
            self.process_direct(content, session_key).await
        }
    }
}
```

**Note on `AgentSession` fields:** `AgentSession` currently has `page_store: Option<Arc<PageStore>>` but no `insight_index` field. The migration plan (Phase 1) adds `insight_index: Option<Arc<InsightIndex>>` to `AgentSession` and wires it through `AgentSessionBuilder`.

---

## Error Handling

| Scenario | Strategy |
|----------|----------|
| Subagent spawn fails | Retry up to 2×, then abort plan and notify user |
| Subagent crash during execution | SQLite fallback allows recovery: read last progress, decide to resume or restart |
| Step execution fails | Mark `status = failed`, record error, retry 3× with exponential backoff (2s/4s/8s), then `step_type = fix` or ask user |
| Dependency step fails | Mark dependent steps `status = skipped`, continue with independent branches |
| Verify FAIL | Extract failure items → append `step_type = fix` steps → re-execute (max `max_retries` cycles) |
| Verify PARTIAL | Ask user to decide: accept, fix, or retry |
| InsightIndex lookup empty | Fallback to Tantivy full-text search; if still empty, proceed without SOP guidance |
| EvolutionHook extraction fails | Log warning, skip memory persistence for this batch, watermark still advances |
| Compactor checkpoint fails | Non-fatal: log warning, continue execution without checkpoint injection |

---

## Testing Strategy

| Module | Test Approach |
|--------|---------------|
| InsightIndex | Unit tests for lookup/upsert/sync; mock PageStore |
| PlanExecutor | Integration tests with mocked SteppableExecutor + MonitoredSpawner |
| ComplexityAssessor | Prompt injection tests: verify correct classification for known inputs |
| MonitoredSpawner | Test channel communication; test SQLite fallback when channel drops |
| MonitoredRunner | Mock LLM + tool registry, verify progress events fire in correct order |
| EvolutionHook | Mock PageStore + InsightIndex, verify SOP created and indexed atomically |
| Compactor Checkpoint | Mock provider, verify checkpoint generated at correct intervals |
| End-to-end | Full PlanExecutor flow with a 3-step task, verify all phases execute |

---

## Migration Plan

### Phase 1: Foundation (No breaking changes)
1. Add `PageType::Sop` variant (+ `as_str()`, `FromStr`, `directory()`)
2. Add `"sops"` directory to `PageStore::init_dirs()`
3. Create `wiki_insights` SQLite table
4. Implement `InsightIndex` module
5. Add `session_checkpoints` table + `CheckpointConfig`
6. Add `insight_index: Option<Arc<InsightIndex>>` field to `AgentSession` (wire through `AgentSessionBuilder`)
7. Add `insight_index` field + `with_insight_index()` builder to `EvolutionHook`

### Phase 2: PlanExecutor Skeleton
1. Create `execution_plans` + `execution_steps` tables
2. Implement `ComplexityAssessor`
3. Implement `PlanExecutor` with Explore + Plan + Execute phases
4. Add `plan_mode` tool to ToolRegistry

### Phase 3: Subagent Monitoring
1. Implement `MonitoredSpawner` + `MonitoredRunner`
2. Wire into PlanExecutor Explore/Verify phases
3. Add `subagent_tasks` SQLite table for fallback

### Phase 4: Evolution + Verify Closure
1. Enhance `EvolutionHook` with SOP extraction + InsightIndex upsert
2. Implement Verify phase in PlanExecutor
3. Add fix loop logic (append new steps with `status = 'pending'` on failure)

### Phase 5: Integration
1. Add `AgentSession.process_with_plan()` and `process_auto()` methods
2. Refactor `KernelExecutor` to use internal `SteppableExecutor`; wire `Compactor.checkpoint()` into PlanExecutor's caller-layer step loop
3. Add user-facing commands (`/plan`, `/verify`)
4. End-to-end testing and documentation

---

## Open Questions

1. **Should `parallel` steps share a subagent provider or use independent instances?** Independent is safer for isolation but costs more tokens.
2. **How should the fix loop interact with the original plan's steps?** Append new steps with `status = 'pending'` or modify existing `status = 'failed'` steps?
3. **Should ComplexityAssessor use the same model as the main agent or a cheaper model?** Using a cheaper model (e.g., Haiku) saves cost but may misclassify.

## References

- GenericAgent repository: https://github.com/lsdefine/GenericAgent
- GenericAgent agent loop: `agent_loop.py` (~100 lines)
- GenericAgent memory SOP: `memory/memory_management_sop.md`
- GenericAgent plan SOP: `memory/plan_sop.md`
- Gasket kernel executor: `gasket/engine/src/kernel/executor.rs`
- Gasket wiki system: `gasket/engine/src/wiki/`
- Gasket hooks: `gasket/engine/src/hooks/`
- Gasket subagents: `gasket/engine/src/subagents/`
