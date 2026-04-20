# GenericAgent-Inspired Evolution Architecture for Gasket

## Status

**Draft** — Pending spec review and user approval.

## Summary

This spec proposes integrating GenericAgent's core innovations — self-evolving skills, structured plan/verify execution, hierarchical memory indexing, and monitored subagent delegation — into gasket's Rust architecture. The design follows a "thin orchestrator" philosophy: a `PlanExecutor` state machine wraps the existing `KernelExecutor` as an optional decorator, leaving the Direct execution path untouched.

Five modules are defined:

1. **InsightIndex** — Wiki navigation layer (L1-equivalent) for O(1) topic-to-SOP routing
2. **PlanExecutor** — Optional 4-phase state machine (Explore/Plan/Execute/Verify) for complex tasks
3. **MonitoredSpawner** — Real-time monitoring and intervention for subagents via channels + SQLite fallback
4. **EvolutionHook Enhancement** — Task-to-SOP crystallization with automatic InsightIndex updates
5. **Compactor Checkpoint** — Proactive working-memory snapshots every N turns

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
│                    │  kernel::    │        │ Subagent    │ │
│                    │  execute()   │        │ (Monitored) │ │
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

1. `PlanExecutor` → `KernelExecutor`: Single-step calls via `ExecutionPlan` step entries
2. `PlanExecutor` → `MonitoredSpawner`: `spawn_monitored()` returns `(handle, interventor, progress_rx)`
3. `PlanExecutor` → `InsightIndex`: `insight.lookup(query, k) -> Vec<InsightEntry>` for SOP routing
4. `EvolutionHook` → `InsightIndex`: Atomic upsert after SOP creation

### Module Dependency Graph

```
PlanExecutor
    ├── uses MonitoredSpawner (Explore, Verify phases)
    ├── uses InsightIndex (Plan phase — SOP lookup)
    ├── uses KernelExecutor (Execute phase — per-step)
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
    cache: RwLock<HashMap<String, Vec<InsightEntry>>>,
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
    pub fn directory(&self) -> &'static str {
        match self {
            Self::Entity => "entities",
            Self::Topic => "topics",
            Self::Source => "sources",
            Self::Sop => "sops",
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
    verification_result TEXT,  -- PASS | FAIL | PARTIAL
    FOREIGN KEY (session_key) REFERENCES sessions(key)
);

CREATE TABLE execution_steps (
    step_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    step_number INTEGER NOT NULL,
    description TEXT NOT NULL,
    marker TEXT NOT NULL DEFAULT '[ ]',  -- [ ], [✓], [✗], [D], [P], [?], [SKIP], [FIX]
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
pub struct ComplexityAssessor;

pub enum Complexity {
    Direct,  // 1-2 steps, skip PlanExecutor
    Auto,    // 3 steps, let agent decide
    Plan,    // 4+ steps or dependencies, force Plan mode
}

impl ComplexityAssessor {
    /// 1-round judgment, ≤100 tokens, uses cheap model
    pub async fn assess(task: &str) -> Complexity;
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
- LLM generates structured plan with `[D]`/`[P]`/`[?]`/`[VERIFY]` markers
- Write plan to wiki + SQLite execution_steps table
- `ask_user` for confirmation before execution

**Execute Phase:**
- For each step in order:
  - Check dependencies satisfied; if not, mark `[SKIP]`
  - `[D]` → delegate to MonitoredSpawner subagent
  - `[P]` → collect parallel subagents, await all
  - default → call KernelExecutor for single-step execution
  - Mini-verify: quick sanity check of output
  - Mark `[✓]` and persist to SQLite

**Verify Phase:**
- Spawn independent verification subagent with adversarial role
- Subagent reads plan + deliverables, runs verification checks
- Subagent outputs `VERDICT: PASS | FAIL | PARTIAL` as final line
- **PASS** → mark `[VERIFY]` as `[✓]`, call EvolutionHook
- **FAIL** → enter `[FIX]` loop (max `max_retries` iterations)
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

        let handle = tokio::spawn(async move {
            let mut runner = MonitoredRunner::new(spec, progress_tx, interventor_rx);
            let result = runner.run().await;
            self.update_db_status(&spec.id, &result).await?;
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
    key_info: String,
    extra_prompt: String,
}

impl MonitoredRunner {
    async fn run(&mut self) -> SubagentResult {
        for turn in 1..=self.spec.max_turns {
            // Check for interventions (non-blocking)
            while let Ok(i) = self.intervention.try_recv() {
                self.apply_intervention(i)?;
            }

            self.progress.send(ProgressUpdate::Thinking { turn }).await.ok();

            let response = self.llm.chat(&self.build_messages()).await?;

            if let Some(tools) = response.tool_calls {
                for tc in tools {
                    self.progress.send(ProgressUpdate::ToolStart { ... }).await.ok();
                    let result = self.execute_tool(&tc).await;
                    self.progress.send(ProgressUpdate::ToolResult { ... }).await.ok();
                }
            }

            self.progress.send(ProgressUpdate::TurnComplete { turn, summary }).await.ok();
        }

        self.progress.send(ProgressUpdate::Done { result }).await.ok();
        SubagentResult::Success(result)
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

`MonitoredRunner` acts as a **decorator** around the existing `run_subagent()` function rather than modifying it directly. This preserves backward compatibility and allows incremental adoption.

---

### 4. EvolutionHook Enhancement

#### Purpose

Extend the existing `EvolutionHook` to classify extracted memories by type, write SOPs as `PageType::Sop`, and atomically update the InsightIndex.

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
    if page_store.read(&path).await.is_ok() {
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

#### Integration with Executor Loop

```rust
// kernel/executor.rs run_loop:

// 1. Existing passive compaction
if token_usage > threshold {
    compactor.try_compact(session_key, token_usage, vault);
}

// 2. NEW: Active checkpoint
if let Some(checkpoint_summary) = compactor
    .checkpoint(session_key, iteration, &recent_events)
    .await?
{
    state.messages.push(ChatMessage::system(format!(
        "[Checkpoint at turn {}]\n{}",
        iteration, checkpoint_summary
    )));
}
```

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
                        │   ├── Route: [D]→subagent, [P]→parallel, default→KernelExecutor
                        │   ├── Mini-verify output
                        │   └── Mark [✓] in SQLite
                        └── All steps done
                        │
                        ▼
                    Verify Phase
                        ├── MonitoredSpawner.spawn(verify_spec)
                        ├── Adversarial verification
                        ├── Parse VERDICT line
                        └── PASS? → Done / FAIL? → [FIX] loop
                        │
                        ▼
                    Done
                        ├── EvolutionHook.on_task_complete()
                        │   ├── Extract verified facts/skills
                        │   ├── Write SOP to wiki (PageType::Sop)
                        │   └── Upsert InsightIndex
                        └── Archive plan to wiki
```

---

## Error Handling

| Scenario | Strategy |
|----------|----------|
| Subagent spawn fails | Retry up to 2×, then abort plan and notify user |
| Subagent crash during execution | SQLite fallback allows recovery: read last progress, decide to resume or restart |
| Step execution fails | Mark `[✗]`, record error, retry 3× with exponential backoff (2s/4s/8s), then `[FIX]` or ask user |
| Dependency step fails | Mark dependent steps `[SKIP]`, continue with independent branches |
| Verify FAIL | Extract failure items → append `[FIX]` steps → re-execute (max `max_retries` cycles) |
| Verify PARTIAL | Ask user to decide: accept, fix, or retry |
| InsightIndex lookup empty | Fallback to Tantivy full-text search; if still empty, proceed without SOP guidance |
| EvolutionHook extraction fails | Log warning, skip memory persistence for this batch, watermark still advances |
| Compactor checkpoint fails | Non-fatal: log warning, continue execution without checkpoint injection |

---

## Testing Strategy

| Module | Test Approach |
|--------|---------------|
| InsightIndex | Unit tests for lookup/upsert/sync; mock PageStore |
| PlanExecutor | Integration tests with mocked KernelExecutor + MonitoredSpawner |
| ComplexityAssessor | Prompt injection tests: verify correct classification for known inputs |
| MonitoredSpawner | Test channel communication; test SQLite fallback when channel drops |
| MonitoredRunner | Mock LLM + tool registry, verify progress events fire in correct order |
| EvolutionHook | Mock PageStore + InsightIndex, verify SOP created and indexed atomically |
| Compactor Checkpoint | Mock provider, verify checkpoint generated at correct intervals |
| End-to-end | Full PlanExecutor flow with a 3-step task, verify all phases execute |

---

## Migration Plan

### Phase 1: Foundation (No breaking changes)
1. Add `PageType::Sop` variant
2. Create `wiki_insights` SQLite table
3. Implement `InsightIndex` module
4. Add `session_checkpoints` table + `CheckpointConfig`

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
3. Add `[FIX]` loop logic

### Phase 5: Integration
1. Wire `PlanExecutor` into `AgentSession.process_direct()`
2. Wire `Compactor.checkpoint()` into `KernelExecutor.run_loop()`
3. Add user-facing commands (`/plan`, `/verify`)
4. End-to-end testing and documentation

---

## Open Questions

1. **Should `[P]` parallel steps share a subagent provider or use independent instances?** Independent is safer for isolation but costs more tokens.
2. **How should the `[FIX]` loop interact with the original plan's steps?** Append new `[FIX]` steps or modify existing `[✗]` steps?
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
