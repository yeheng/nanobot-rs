# GenericAgent-Inspired Evolution Architecture for Gasket

## Status

**Draft** — Pending spec review and user approval.

## Summary

This spec proposes integrating GenericAgent's core innovations — self-evolving skills, structured plan execution, monitored subagent delegation, and proactive working memory — into gasket's Rust architecture. The design follows a **tool-based philosophy**: instead of hardcoding a workflow engine, we expose capabilities as **tools** that the LLM decides when to invoke.

Four modules are defined:

1. **SteppableExecutor** — Foundation primitive: per-step LLM execution extracted from `KernelExecutor`
2. **SOP + Tantivy Discovery** — Add `PageType::Sop` to wiki; LLM self-routes via existing Tantivy search
3. **MonitoredSpawner** — Real-time monitoring and intervention for subagents via channels (no DB fallback)
4. **EvolutionHook Enhancement** — Task-to-SOP crystallization with "No Execution, No Memory" enforcement
5. **Compactor Checkpoint** — Proactive working-memory snapshots every N turns at the caller layer

## Key Design Decision: Tool-Based Architecture

The original design proposed a `PlanExecutor` state machine with `ComplexityAssessor` routing. After review, this was identified as **over-engineering**:

- **Problem**: Hardcoding a 4-phase FSM (Explore/Plan/Execute/Verify) creates two parallel execution paths and adds an extra LLM call just for routing.
- **Solution**: Provide a `create_plan` **tool**. If the LLM decides a task is complex, it calls the tool. The tool returns structured steps into the message history. The agent then executes them using the same `KernelExecutor` loop — no separate path, no `ComplexityAssessor`.

This is the Unix philosophy: small, composable tools. The LLM is the orchestrator, not a Rust FSM.

## Key Design Decision: Steppable Execution Primitive

`KernelExecutor` runs a **full autonomous loop** (`for iteration in 1..=max_iterations`) with no public API for per-step external orchestration. To enable monitored subagent execution and future caller-layer checkpointing, we introduce **`SteppableExecutor`**:

- `SteppableExecutor` is a **refactored extraction** from `KernelExecutor` that splits the loop body into discrete `step()` calls
- `KernelExecutor` internally uses `SteppableExecutor` (composition, not inheritance), preserving its existing API
- `MonitoredRunner` calls `SteppableExecutor::step()` for each turn, emitting progress events
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
- Wiki has search (Tantivy) but no **SOP-aware routing** for LLM self-discovery
- EvolutionHook extracts facts but not **action-verified procedural skills (SOPs)**
- ContextCompactor is passive — no proactive working-memory checkpoints

## Goals

1. Let the agent **plan complex tasks** by providing a `create_plan` tool — the LLM decides when to use it
2. Let the agent **discover and reuse its own skills** via `PageType::Sop` pages discoverable through Tantivy
3. Provide real-time oversight of subagent execution with intervention capabilities
4. Ensure task learnings are **automatically crystallized** into reusable SOPs
5. Maintain proactive working memory without heuristic turn-based patches

## Non-Goals

- Replacing the existing Direct execution mode — the unified path stays; `create_plan` is just another tool
- Porting GenericAgent's Python implementation verbatim — we adapt concepts to Rust idioms
- Supporting GenericAgent's browser automation (webdriver) — out of scope
- Changing the Actor pipeline architecture (Router→Session→Outbound)
- Building a workflow engine with DB state tables — plans live in message history, not SQLite

## Changes from Review (Iteration 1)

The following critical issues were identified and fixed during the first spec review:

1. **KernelExecutor not steppable** — Spec originally assumed per-step calls, but `KernelExecutor.run_loop()` runs a full autonomous loop. **Fix**: Introduced `SteppableExecutor` as a new primitive.

2. **AgentSession.process_direct() can't be branched** — No hook point existed for `ComplexityAssessor` before kernel runs. **Fix**: Added new `AgentSession.process_with_plan()` method plus `process_auto()` convenience wrapper.

3. **SQLite FK to non-existent sessions table** — `execution_plans` had `FOREIGN KEY REFERENCES sessions(key)` but no `sessions` table exists. **Fix**: Removed FK.

4. **MonitoredSpawner can't decorate runner.rs** — `run_subagent()` is a pure function with no internal hooks. **Fix**: `MonitoredRunner` implements its own execution loop using `SteppableExecutor`.

5. **Missing sops/ directory** — `PageStore::init_dirs()` doesn't create "sops". **Fix**: Added to Migration Plan Phase 1.

## Changes from Review (Iteration 2)

1. **Checkpoint injection at wrong layer** — `BeforeLLM` hook fires once per request. **Fix**: Checkpoint injection moved to the **caller layer**.
2. **MonitoredSpawner::spawn() borrow checker error** — **Fix**: Clone `sqlite_pool` before spawn.
3. **PageType::Sop missing `as_str()` and `FromStr`** — **Fix**: Added both.
4. **MonitoredRunner missing tools/provider fields** — **Fix**: Uses `SteppableExecutor` internally.
5. **SteppableExecutor missing `TokenLedger`** — **Fix**: Added `ledger` parameter to `step()`.

## Changes from Review (Iteration 3)

1. **ProgressUpdate field mismatches** — **Fix**: Simplified enum variants to match actual usage.
2. **Undefined `turn` variable** — **Fix**: Added `let mut turn = 0` counter.
3. **`ask_user` ambiguity** — **Fix**: Clarified as `confirm_plan` tool call.

## Changes from Linus Review (Major Simplification)

The Linus-style review in `task.md` identified fundamental over-engineering. The following **large-scale cuts** were made:

1. **Killed `ComplexityAssessor`** — Extra LLM call just for routing. The LLM itself decides when to plan via the `create_plan` tool.
2. **Killed hardcoded `PlanExecutor` FSM** — Replaced with `create_plan` tool. Plans live in EventStore message history, not SQLite tables.
3. **Killed `execution_plans` + `execution_steps` tables** — Temporary execution state belongs in the context, not a relational schema.
4. **Killed `wiki_insights` table** — `InsightIndex` was a parallel projection of `wiki_pages`. SOPs are discovered through existing Tantivy search on `PageType::Sop`.
5. **Killed `subagent_tasks` SQLite fallback** — If a subagent crashes, the user re-sends the instruction. KISS.
6. **Kept `SteppableExecutor`** — Foundation primitive for per-step control. "The most tasteful design in the document."
7. **Kept `MonitoredSpawner`** — Channel-based progress/intervention is genuinely useful for UI feedback.
8. **Kept `EvolutionHook` SOP extraction** — Auto-crystallization of skills is the core value proposition.
9. **Kept `Checkpoint`** — Proactive working memory at the caller layer.

## Architecture

### Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        AgentSession                           │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │              KernelExecutor.run_loop()               │   │
│  │  ┌────────────┐  ┌──────────┐  ┌─────────────────┐  │   │
│  │  │ Steppable  │  │ create_  │  │  spawn_         │  │   │
│  │  │ Executor   │  │ plan     │  │  monitored      │  │   │
│  │  │  (tool)    │  │  (tool)  │  │  (tool)         │  │   │
│  │  └────────────┘  └──────────┘  └─────────────────┘  │   │
│  │         ▲                                    │       │   │
│  └─────────┼────────────────────────────────────┼───────┘   │
│            │                                    │            │
│  ┌─────────┴──────────┐              ┌──────────┴────────┐  │
│  │   Tantivy Search   │              │ MonitoredRunner   │  │
│  │   (PageType::Sop)  │              │ (progress +       │  │
│  │                    │              │  intervention)    │  │
│  └─────────┬──────────┘              └──────────┬────────┘  │
│            │                                    │            │
│  ┌─────────┴──────────┐              ┌──────────┴────────┐  │
│  │    PageStore       │              │  EvolutionHook    │  │
│  │  (wiki: sops/)     │              │  (SOP extraction) │  │
│  └────────────────────┘              └───────────────────┘  │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │           ContextCompactor.checkpoint()              │   │
│  │     (called by AgentSession between turns)           │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

**Key principle**: One unified execution path. The LLM uses tools (`create_plan`, `spawn_monitored`) when it decides they are needed. No routing layer, no FSM.

### Module Dependency Graph

```
KernelExecutor
    ├── internally uses SteppableExecutor (surgical refactor)
    └── exposes tools: create_plan, spawn_monitored, search_sops

MonitoredSpawner
    ├── wraps SimpleSpawner
    ├── uses tokio::sync::mpsc (channels only, no DB fallback)
    └── MonitoredRunner uses SteppableExecutor internally

EvolutionHook (enhanced)
    ├── uses PageStore (write SOP pages as PageType::Sop)
    └── triggered at AfterResponse hook point

TantivyAdapter
    ├── already indexes all wiki pages
    └── SOPs discovered via search with PageType::Sop filter

Compactor (enhanced)
    ├── adds checkpoint() method
    ├── writes session_checkpoints table
    └── called from AgentSession every N turns
```

## Detailed Design

### 1. SOP Discovery via Tantivy

#### Purpose

Instead of building a parallel `wiki_insights` table, we leverage the **existing Tantivy index**. Adding `PageType::Sop` to the wiki makes SOPs searchable through the same infrastructure already used for entities, topics, and sources.

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
            Self::Sop => "sop",
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
            "sop" => Ok(Self::Sop),
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

#### Discovery: `search_sops` Tool

The agent discovers its own SOPs via a tool that queries Tantivy:

```rust
/// Tool: search_sops — find relevant SOPs by query string
pub async fn search_sops(query: &str, k: usize) -> Vec<SearchHit> {
    // Query Tantivy with PageType::Sop filter
    let mut filter = PageFilter::default();
    filter.page_type = Some(PageType::Sop);
    page_index.search(query, k, Some(filter)).await
}
```

This tool is registered in `ToolRegistry` and available to the LLM during any turn. When the agent encounters a task it has an SOP for, it can retrieve and follow it.

#### Why Not a Separate InsightIndex Table?

- **Tantivy already indexes everything** — adding a parallel SQLite table for "topic → path" mapping is duplication
- **SOPs are content** — they have titles, tags, and full text. Tantivy BM25 scoring is superior to a flat `relevance` field
- **KISS** — one search index, not two

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

**Note on `ToolContext`:** The actual `KernelExecutor` constructs `ToolContext` with `spawner` and `token_tracker` references for tool execution. `SteppableExecutor` must also accept these (or equivalents) so that tool calls inside `step()` have access to subagent spawning and token tracking. The spec pseudocode omits these for brevity; they are required fields on `SteppableExecutor`.

---

### 2. create_plan Tool

#### Purpose

Instead of a hardcoded `PlanExecutor` FSM, we provide a **`create_plan` tool** that the LLM can call when it decides a task is complex. The tool generates structured steps, writes them to the wiki, and returns the plan to the LLM. The agent then executes the steps using the normal `KernelExecutor` loop — the same path as any other task.

#### Why a Tool Instead of a State Machine?

- **No routing overhead** — No extra LLM call (`ComplexityAssessor`) to decide whether to plan
- **Single execution path** — Direct tasks and planned tasks use the same `KernelExecutor` loop
- **LLM-driven** — The agent decides when planning is useful, not a heuristic
- **Simpler** — No FSM, no DB tables for plan state, no `process_with_plan()` branching

#### Tool Definition

```rust
/// Tool: create_plan — generate a structured execution plan for a complex task
///
/// The LLM calls this when it determines a task requires multiple steps.
/// The plan is persisted to the wiki and returned as a structured message.
pub struct CreatePlanTool {
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Arc<PageStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    pub step_type: StepType,  // Direct | Delegated | Parallel | Conditional
    pub depends_on: Vec<usize>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    Direct,     // Execute via single SteppableExecutor step
    Delegated,  // Spawn MonitoredSpawner subagent
    Parallel,   // Execute alongside other steps
    Conditional, // Execute only if condition evaluates true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub verification_criteria: Vec<String>,
    pub wiki_path: String,
}

impl CreatePlanTool {
    pub async fn invoke(&self, goal: &str, context: &[ChatMessage]) -> Result<Plan, ToolError> {
        // 1. Search for relevant SOPs
        let sops = self.search_relevant_sops(goal).await?;

        // 2. Build prompt with SOP context + plan generation instructions
        let prompt = self.build_plan_prompt(goal, &sops, context);

        // 3. Call LLM to generate structured plan
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage::system("You are a planning assistant."), prompt],
            max_tokens: Some(2048),
            temperature: Some(0.3),
            ..Default::default()
        };

        let response = self.provider.chat(request).await?;
        let plan: Plan = self.parse_plan(&response.content.unwrap_or_default())?;

        // 4. Persist plan to wiki
        self.page_store.write(&plan.to_wiki_page()).await?;

        Ok(plan)
    }
}
```

#### Plan Execution Flow

The plan is **not executed by a separate engine**. It is injected into the message history as a structured artifact. The agent then proceeds with normal execution:

```
User: "Set up a new Rust project with CI, tests, and documentation"

Agent (turn 1):
  LLM thinks: "This is complex, I'll create a plan"
  → Calls create_plan tool
  → Tool returns Plan { steps: [...] }
  → Plan written to wiki: "plans/rust_project_setup"
  → LLM responds: "I've created a plan with 4 steps. Let me start..."

Agent (turn 2):
  LLM sees plan in context
  → Executes step 1 (Direct): "cargo init"
  → Tool result: project created

Agent (turn 3):
  → Executes step 2 (Delegated): spawn_monitored for CI setup
  → MonitoredSpawner reports progress
  → Subagent completes

Agent (turn 4):
  → Executes step 3 (Direct): add tests
  → ...

Agent (turn N):
  → All steps complete
  → LLM calls verify_completion tool (optional)
  → Responds to user
```

#### Plan Persistence

Plans are stored as `PageType::Topic` wiki pages (not in SQLite tables). The path follows `plans/{slug}` convention. This keeps all knowledge in one system — the wiki.

---

### 3. MonitoredSpawner Module

#### Purpose

Add real-time monitoring and intervention to subagent execution. Replaces GenericAgent's file-IO protocol (`_intervene`, `_stop`, `_keyinfo`) with type-safe Rust channels.

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
    ToolStart { name: String },
    ToolResult { name: String, output: String },
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

#### Implementation (Channels Only, No DB Fallback)

```rust
pub struct MonitoredSpawner {
    inner: SimpleSpawner,
}

impl MonitoredSpawner {
    pub fn new(inner: SimpleSpawner) -> Self {
        Self { inner }
    }

    pub async fn spawn(&self, spec: TaskSpec) -> Result<MonitoredHandle> {
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (interventor_tx, interventor_rx) = mpsc::channel(16);

        // Build SteppableExecutor from spec
        let steppable = SteppableExecutor::new(
            spec.provider.clone(),
            spec.tools.clone(),
            spec.config.clone(),
        );

        let handle = tokio::spawn(async move {
            let mut runner = MonitoredRunner::new(
                spec,
                steppable,
                progress_tx,
                interventor_rx,
            );
            runner.run().await
        });

        Ok(MonitoredHandle {
            handle,
            interventor: interventor_tx,
            progress: progress_rx,
        })
    }
}
```

**No SQLite fallback.** If the subagent crashes, state is lost. The user re-sends the instruction. This is the KISS principle — the complexity of crash recovery outweighs the benefit for a local personal AI assistant.

#### MonitoredRunner

```rust
struct MonitoredRunner {
    spec: TaskSpec,
    steppable: SteppableExecutor,
    messages: Vec<ChatMessage>,
    ledger: TokenLedger,
    progress: mpsc::Sender<ProgressUpdate>,
    intervention: mpsc::Receiver<Intervention>,
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
            let result = self.steppable.step(
                &mut self.messages,
                &mut self.ledger,
                None,
            ).await?;

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

#### Integration with Existing runner.rs

**Important**: The existing `run_subagent()` in `runner.rs` is a **pure function** that creates a `KernelExecutor` and runs it to completion. It has no internal hooks for progress observation or intervention.

Therefore, `MonitoredRunner` is **not a decorator** around `run_subagent()`. Instead:

1. `MonitoredRunner` implements its own execution loop using `SteppableExecutor`
2. Progress events are emitted at each turn boundary
3. Interventions are checked between turns via `try_recv()`
4. The existing `SimpleSpawner` API remains unchanged for non-monitored use cases

---

### 4. EvolutionHook Enhancement

#### Purpose

Extend the existing `EvolutionHook` to classify extracted memories by type and write SOPs as `PageType::Sop`.

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
            "skill" => self.persist_as_sop(mem).await,    // L3: Sop
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

    // Tantivy re-index happens automatically via PageIndex
    // No separate InsightIndex table needed

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
```

**Integration with `ContextCompactor`:** `CheckpointConfig` is passed to `ContextCompactor` at construction time (e.g., as a new parameter to `ContextCompactor::new()` or via a `with_checkpoint_config()` builder). The existing `ContextCompactor` already has `provider`, `event_store`, `sqlite_store`, `model`, and `token_budget`; `CheckpointConfig` becomes an additional optional field. If `None`, checkpointing is disabled and only passive compaction runs.

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

Instead, checkpoint injection happens at the **caller layer**:

**Direct mode (`KernelExecutor`)**: No proactive checkpoint injection. Only existing passive compaction (`try_compact` when token threshold is exceeded) is active. Proactive checkpoints require the steppable executor.

**Monitored subagent mode (`SteppableExecutor`)**: `MonitoredRunner` can optionally call `compactor.checkpoint()` between `step()` calls and inject the result into the message history. This is an optional enhancement.

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
KernelExecutor.run_loop() ── LLM decides how to proceed
    │
    ├── Simple task ──► Execute normally ──► Done
    │
    └── Complex task ──► LLM calls create_plan tool
            │
            ▼
        create_plan
            ├── Search Tantivy for relevant SOPs (PageType::Sop)
            ├── LLM generates structured plan
            ├── Persist plan to wiki (PageType::Topic)
            └── Return plan to agent context
            │
            ▼
        Agent continues execution
            ├── Step 1: Direct → SteppableExecutor.step()
            ├── Step 2: Delegated → MonitoredSpawner.spawn()
            │   ├── ProgressUpdate events stream back
            │   └── Intervention possible
            ├── Step 3: Parallel → Multiple subagents
            └── ...
            │
            ▼
        Done
            ├── EvolutionHook.on_task_complete()
            │   ├── Extract verified facts/skills
            │   └── Write SOP to wiki (PageType::Sop)
            └── Respond to user
```

---

## Error Handling

| Scenario | Strategy |
|----------|----------|
| Subagent spawn fails | Retry up to 2×, then abort and return error to LLM (which can decide next action) |
| Subagent crash during execution | State lost. User re-sends instruction. No DB fallback. |
| Plan generation fails | Return error to LLM; agent can retry or proceed without plan |
| Step execution fails | LLM sees tool error in context, decides retry/skip/abort |
| Verify FAIL | LLM sees result, decides fix approach (no hardcoded fix loop) |
| SOP search empty | Proceed without SOP guidance; agent improvises |
| EvolutionHook extraction fails | Log warning, skip memory persistence for this batch, watermark still advances |
| Compactor checkpoint fails | Non-fatal: log warning, continue execution without checkpoint injection |

---

## Testing Strategy

| Module | Test Approach |
|--------|---------------|
| SteppableExecutor | Existing agent chat tests must pass 100%. Verify TokenLedger accumulates correctly. |
| PageType::Sop | `PageType::from_str("sop")` returns `Ok(PageType::Sop)`. Wiki init creates `sops/` dir. |
| EvolutionHook SOP | Mock LLM returns `skill` type JSON; verify `PageStore::write` called with `PageType::Sop`. |
| MonitoredSpawner | Test channel communication. Send `Intervention::Abort`, verify subagent exits cleanly. |
| MonitoredRunner | Mock LLM + tool registry, verify progress events fire in correct order. |
| Compactor Checkpoint | Mock provider, verify checkpoint generated at correct intervals. |
| create_plan Tool | Test that tool returns structured Plan; verify persisted to wiki. |
| End-to-end | Agent uses create_plan for a 3-step task, executes all steps, SOP extracted. |

---

## Migration Plan

### Task 1: Extract SteppableExecutor
1. Define `StepResult` struct
2. Extract `KernelExecutor` loop body into `SteppableExecutor::step(&mut messages, &mut ledger, event_tx)`
3. Rewrite `KernelExecutor::run_loop()` to call `step()` in a loop

**Acceptance Criteria**: All existing agent chat tests pass. `KernelExecutor` external API unchanged.

### Task 2: Extend Wiki for SOP
1. Add `PageType::Sop` variant (+ `as_str()`, `FromStr`, `directory()`)
2. Add `"sops"` directory to `PageStore::init_dirs()`
3. Register `search_sops` tool in `ToolRegistry`

**Acceptance Criteria**: `PageType::from_str("sop")` works. `sops/` directory created on init.

### Task 3: Enhance EvolutionHook for SOP
1. Modify extraction prompt for "No Execution, No Memory" + `note`/`skill` classification
2. Modify `persist_memory` to route `skill` → `persist_as_sop()`
3. Write SOP pages with proper frontmatter + body structure

**Acceptance Criteria**: Mock LLM returns `skill` type → `PageStore::write` called with `PageType::Sop`.

### Task 4: Implement MonitoredSpawner
1. Define `ProgressUpdate` and `Intervention` enums
2. Create `MonitoredRunner` wrapping `SteppableExecutor`
3. Add `spawn_monitored` to `SubagentSpawner` trait

**Acceptance Criteria**: `Intervention::Abort` causes subagent to exit after current step.

### Task 5: Implement Checkpoint
1. Add `checkpoint()` method to `ContextCompactor`
2. Create `session_checkpoints` SQLite table
3. Wire `CheckpointConfig` into `ContextCompactor`
4. Call from `AgentSession` when driving `SteppableExecutor` (e.g., in monitored subagent mode)

**Acceptance Criteria**: Checkpoint inserted into `session_checkpoints` at correct interval.

### Bonus: create_plan Tool (after core tasks)
1. Implement `CreatePlanTool` with SOP search + plan generation
2. Register in `ToolRegistry`
3. Test end-to-end planning flow

---

## Open Questions

1. **Should `parallel` steps share a subagent provider or use independent instances?** Independent is safer for isolation but costs more tokens.
2. **How detailed should `create_plan` output be?** Full step-by-step with tool calls, or high-level goals left to LLM improvisation?
3. **Should checkpointing be enabled for direct mode too?** Requires adding a `BeforeIteration` hook point to `KernelExecutor`.

## References

- GenericAgent repository: https://github.com/lsdefine/GenericAgent
- GenericAgent agent loop: `agent_loop.py` (~100 lines)
- GenericAgent memory SOP: `memory/memory_management_sop.md`
- GenericAgent plan SOP: `memory/plan_sop.md`
- Gasket kernel executor: `gasket/engine/src/kernel/executor.rs`
- Gasket wiki system: `gasket/engine/src/wiki/`
- Gasket hooks: `gasket/engine/src/hooks/`
- Gasket subagents: `gasket/engine/src/subagents/`
