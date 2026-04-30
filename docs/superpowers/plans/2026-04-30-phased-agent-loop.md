# Phased Agent Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a phased agent execution loop (Research → Planning → Execute → Review) with engine-enforced auto-search and cross-session learning, gated behind a config flag for backward compatibility.

**Architecture:** A new `PhasedExecutor` wraps the existing `SteppableExecutor`, managing phase state and injecting phase-aware prompts. A `PhaseTransitionTool` (internal, not publicly registered) lets the LLM declare phase changes. The Research phase auto-runs `wiki_search` + `history_search` via the ToolRegistry before the LLM sees the user message. A `PhaseChanged` ChatEvent variant notifies the frontend.

**Tech Stack:** Rust (tokio async), gasket-types, gasket-providers, ChatEvent WebSocket protocol, Vue.js frontend

---

## File Structure

| File | Action | Purpose |
|------|--------|---------|
| `gasket/types/src/phase.rs` | Create | `AgentPhase` enum + display logic |
| `gasket/types/src/lib.rs` | Modify | Re-export `phase` module |
| `gasket/types/src/events/stream.rs` | Modify | Add `PhaseChanged` ChatEvent variant |
| `gasket/engine/src/tools/phase_transition.rs` | Create | `PhaseTransitionTool` — internal tool for LLM phase switching |
| `gasket/engine/src/tools/mod.rs` | Modify | Register `phase_transition` module |
| `gasket/engine/src/kernel/context.rs` | Modify | Add `phased` to `KernelConfig` |
| `gasket/engine/src/kernel/phased_executor.rs` | Create | `PhasedExecutor` — state machine wrapping SteppableExecutor |
| `gasket/engine/src/kernel/executor.rs` | Modify | Re-export `PhasedExecutor` |
| `gasket/engine/src/kernel/mod.rs` | Modify | Register `phased_executor` module |
| `web/src/composables/useChatSession.ts` | Modify | Handle `phase_changed` events |
| `web/src/components/ChatMessage.vue` | Modify | Render phase indicator badge |

## Simplifications vs. Spec

Two spec requirements are deferred to a follow-up:

1. **Tool filtering by phase (Spec 2.4)**: Phase 1 uses prompt guidance only — the LLM is told which tools to use but not prevented from calling others. Keeps the ToolRegistry unchanged.
2. **User clarification re-entrancy (Spec 3.4)**: The PhasedExecutor is structured as re-entrant (`cycle()` returns `needs_user_input`), but the kernel entry creates a fresh executor each call. Full re-entrancy requires session dispatcher changes — deferred.

---

### Task 1: AgentPhase type in gasket-types

**Files:**
- Create: `gasket/types/src/phase.rs`
- Modify: `gasket/types/src/lib.rs` (add `pub mod phase;`)

- [ ] **Step 1: Write the phase module**

```rust
// gasket/types/src/phase.rs
use serde::{Deserialize, Serialize};

/// Agent execution phase in the phased execution model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    /// Engine-enforced: auto-search wiki + history, retrieval sub-loop, user clarification
    Research,
    /// LLM-driven: create plan based on gathered context
    Planning,
    /// LLM-driven: full tool execution (standard SteppableExecutor behavior)
    Execute,
    /// LLM-driven: review results, extract learnings, write wiki
    Review,
    /// Terminal state
    Done,
}

impl AgentPhase {
    /// Display label for frontend phase indicator
    pub fn label(&self) -> &'static str {
        match self {
            AgentPhase::Research => "Research",
            AgentPhase::Planning => "Planning",
            AgentPhase::Execute => "Execute",
            AgentPhase::Review => "Review",
            AgentPhase::Done => "Done",
        }
    }

    /// Valid target phases that can be transitioned to from this phase
    pub fn valid_targets(&self) -> &'static [AgentPhase] {
        match self {
            AgentPhase::Research => &[AgentPhase::Planning, AgentPhase::Execute],
            AgentPhase::Planning => &[AgentPhase::Execute],
            AgentPhase::Execute => &[AgentPhase::Review, AgentPhase::Done],
            AgentPhase::Review => &[AgentPhase::Done],
            AgentPhase::Done => &[],
        }
    }

    /// Whether this phase allows transition to the given target
    pub fn can_transition_to(&self, target: AgentPhase) -> bool {
        self.valid_targets().contains(&target)
    }
}

impl std::fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

In `gasket/types/src/lib.rs`, find the existing `pub mod` declarations and add:

```rust
pub mod phase;
```

Re-export at the top of lib.rs alongside existing re-exports:

```rust
pub use phase::AgentPhase;
```

- [ ] **Step 3: Build to verify**

```bash
cargo build --package gasket-types
```

Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add gasket/types/src/phase.rs gasket/types/src/lib.rs
git commit -m "feat(types): add AgentPhase enum for phased execution model"
```

---

### Task 2: PhaseChanged ChatEvent variant

**Files:**
- Modify: `gasket/types/src/events/stream.rs` (add variant + constructor)

- [ ] **Step 1: Add PhaseChanged variant to ChatEvent**

In `gasket/types/src/events/stream.rs`, add to the `ChatEvent` enum (after the existing variants, before the closing `}`):

```rust
    /// Agent transitioned to a new execution phase
    PhaseChanged { phase: crate::AgentPhase },
```

- [ ] **Step 2: Add constructor**

In the `impl ChatEvent` block, add after the existing constructor methods:

```rust
    /// Create a phase_changed message
    pub fn phase_changed(phase: crate::AgentPhase) -> Self {
        Self::PhaseChanged { phase }
    }
```

- [ ] **Step 3: Build and run existing tests**

```bash
cargo build --package gasket-types
cargo test --package gasket-types
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add gasket/types/src/events/stream.rs
git commit -m "feat(types): add PhaseChanged ChatEvent for frontend phase indicator"
```

---

### Task 3: PhaseTransitionTool

**Files:**
- Create: `gasket/engine/src/tools/phase_transition.rs`
- Modify: `gasket/engine/src/tools/mod.rs` (add module + re-export)

- [ ] **Step 1: Write PhaseTransitionTool**

```rust
// gasket/engine/src/tools/phase_transition.rs
//! Internal tool for LLM-driven phase transitions.
//!
//! This tool is NOT registered in the public ToolRegistry. It is attached
//! only by the PhasedExecutor during phased execution.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolResult};
use gasket_types::AgentPhase;

/// Internal tool that allows the LLM to declare phase transitions.
///
/// The current phase is shared via `Arc<Mutex<AgentPhase>>` so the
/// PhasedExecutor can detect transitions after each step.
pub struct PhaseTransitionTool {
    current_phase: Arc<Mutex<AgentPhase>>,
}

impl PhaseTransitionTool {
    pub fn new(current_phase: Arc<Mutex<AgentPhase>>) -> Self {
        Self { current_phase }
    }
}

#[derive(Deserialize)]
struct TransitionArgs {
    phase: AgentPhase,
    #[serde(default)]
    context_summary: Option<String>,
}

#[async_trait]
impl Tool for PhaseTransitionTool {
    fn name(&self) -> &str {
        "phase_transition"
    }

    fn description(&self) -> &str {
        "Transition to the next working phase. Call this when you have gathered \
         enough information (Research), completed planning (Planning), finished \
         execution (Execute), or completed review (Review)."
    }

    fn parameters(&self) -> Value {
        let phase = self.current_phase.lock().unwrap();
        let valid: Vec<&str> = phase
            .valid_targets()
            .iter()
            .map(|p| match p {
                AgentPhase::Planning => "planning",
                AgentPhase::Execute => "execute",
                AgentPhase::Review => "review",
                AgentPhase::Done => "done",
                _ => "unknown",
            })
            .collect();

        serde_json::json!({
            "type": "object",
            "properties": {
                "phase": {
                    "type": "string",
                    "enum": valid,
                    "description": "Target phase to transition to"
                },
                "context_summary": {
                    "type": "string",
                    "description": "Optional summary of findings for the next phase"
                }
            },
            "required": ["phase"]
        })
    }

    #[instrument(name = "tool.phase_transition", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: TransitionArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let mut phase = self.current_phase.lock().unwrap();
        let current = *phase;

        if !current.can_transition_to(parsed.phase) {
            return Err(ToolError::InvalidArguments(format!(
                "Cannot transition from {} to {}. Valid targets: {:?}",
                current.label(),
                parsed.phase.label(),
                current.valid_targets().iter().map(|p| p.label()).collect::<Vec<_>>()
            )));
        }

        debug!(
            "Phase transition: {} -> {} (summary: {:?})",
            current.label(),
            parsed.phase.label(),
            parsed.context_summary
        );

        *phase = parsed.phase;

        let summary = parsed
            .context_summary
            .map(|s| format!("\nContext summary: {}", s))
            .unwrap_or_default();

        Ok(format!(
            "Transitioned from {} to {}.{}",
            current.label(),
            parsed.phase.label(),
            summary
        ))
    }
}
```

- [ ] **Step 2: Register module in tools/mod.rs**

Add to the module declarations (alphabetically, after `new_session`):

```rust
mod phase_transition;
```

Add to the re-exports (alongside other tool types):

```rust
pub use phase_transition::PhaseTransitionTool;
```

- [ ] **Step 3: Build to verify**

```bash
cargo build --package gasket-engine
```

Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/tools/phase_transition.rs gasket/engine/src/tools/mod.rs
git commit -m "feat(engine): add PhaseTransitionTool for LLM-driven phase switching"
```

---

### Task 4: PhasedExecutor — state machine (re-entrant design)

**Files:**
- Create: `gasket/engine/src/kernel/phased_executor.rs`
- Modify: `gasket/engine/src/kernel/executor.rs` (add re-export)
- Modify: `gasket/engine/src/kernel/mod.rs` (add module)

- [ ] **Step 1: Write the PhasedExecutor**

```rust
// gasket/engine/src/kernel/phased_executor.rs
//! Phased executor — Research → Planning → Execute → Review state machine.
//!
//! Wraps `SteppableExecutor`, managing phase state and injecting phase-aware
//! prompts.  Designed to be **re-entrant**: each user message drives one
//! `cycle()` call, and the executor yields control when the LLM asks the user
//! a question mid-Research (or when the full phase chain completes).
//!
//! The caller (session dispatcher / kernel_executor) handles the I/O boundary:
//! call `cycle()` → if `needs_user_input`, send response to user, wait for
//! their reply, append it to messages, call `cycle()` again without re-running
//! auto-search.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::kernel::{
    context::RuntimeContext,
    error::KernelError,
    steppable_executor::SteppableExecutor,
};
use crate::tools::PhaseTransitionTool;
use gasket_providers::ChatMessage;
use gasket_types::AgentPhase;

/// Maximum iterations per phase.
const MAX_RESEARCH_ITERS: u32 = 5;
const MAX_PLANNING_ITERS: u32 = 3;
const MAX_REVIEW_ITERS: u32 = 3;

// ── Phase prompts ───────────────────────────────────────────

const RESEARCH_PROMPT: &str = "\
[Phase: Research]

You are in the Research phase. Gather information to understand the user's request:
- Use `wiki_search` and `wiki_read` to find relevant knowledge
- Use `history_search` and `query_history` to recall past conversations
- If information is insufficient, ask the user clarifying questions
- When you have enough context, call `phase_transition(\"planning\")` to plan, \
  or `phase_transition(\"execute\")` to skip planning for simple tasks.

**Important:** If you respond with text (not a tool call) during Research, \
your response will be sent to the user and you will wait for their reply. \
Only respond with text when you really need the user's input.";

const PLANNING_PROMPT: &str = "\
[Phase: Planning]

Research findings: {context_summary}

Based on the research and user's request, create a plan.
Call `phase_transition(\"execute\")` when ready, or skip with a direct transition \
for simple tasks.";

const EXECUTE_PROMPT: &str = "\
[Phase: Execute]

Execute your plan. All tools are now available.
When done, call `phase_transition(\"review\")` for reflection, \
or `phase_transition(\"done\")` to finish.";

const REVIEW_PROMPT: &str = "\
[Phase: Review]

Review the execution. Ask yourself:
1. Did you achieve the user's goal?
2. What persistent knowledge should be saved?
3. Which wiki pages should be created or updated?

If you found valuable knowledge, write it to wiki using `wiki_write`.
Call `phase_transition(\"done\")` when finished.";

// ── Result types ─────────────────────────────────────────────

/// Outcome of one phased cycle.
pub struct PhasedCycleResult {
    /// Text response to send to the user (only set when needs_user_input is true).
    pub user_response: Option<String>,
    /// Whether execution is complete.
    pub done: bool,
    /// Final content (only set when done is true).
    pub content: Option<String>,
    /// Final reasoning (only set when done is true).
    pub reasoning_content: Option<String>,
    /// Tool names used across all phases in this cycle.
    pub tools_used: Vec<String>,
    /// Phases visited during this cycle.
    pub phases_visited: Vec<AgentPhase>,
    /// Whether the executor needs the user to respond before continuing.
    /// Caller should send `user_response` to the user, wait for input,
    /// append the user's message to the message list, and call `cycle()` again.
    pub needs_user_input: bool,
}

/// Re-entrant phased executor.
///
/// Owns the phase state across multiple `cycle()` calls.  Each `cycle()`
/// advances the phase machine as far as possible, yielding only when the
/// LLM asks the user a question (mid-Research) or when the full chain
/// reaches Done.
pub struct PhasedExecutor {
    ctx: RuntimeContext,
    phase: Arc<Mutex<AgentPhase>>,
    context_summary: Arc<Mutex<Option<String>>>,
    /// True after the first auto-search has run — prevents re-running on re-entry.
    auto_search_done: bool,
    /// Messages accumulated across cycles (the PhasedExecutor owns the history).
    messages: Vec<ChatMessage>,
}

impl PhasedExecutor {
    /// Create a new PhasedExecutor from the given context and initial messages.
    ///
    /// The `messages` should include the system prompt and the first user message.
    /// Builds an internal ToolRegistry that adds `phase_transition` alongside
    /// the original tools.
    pub fn new(ctx: RuntimeContext, messages: Vec<ChatMessage>) -> Self {
        let phase = Arc::new(Mutex::new(AgentPhase::Research));
        let context_summary = Arc::new(Mutex::new(None::<String>));

        // Clone original registry and attach phase_transition
        let mut registry = (*ctx.tools).clone();
        registry.register(Box::new(PhaseTransitionTool::new(phase.clone())));

        let mut ctx = ctx;
        ctx.tools = Arc::new(registry);

        Self {
            ctx,
            phase,
            context_summary,
            auto_search_done: false,
            messages,
        }
    }

    /// Run one cycle of the phased execution.
    ///
    /// On first call: runs auto-search, injects Research prompt, enters the
    /// phase loop.  On subsequent calls (after user input): resumes from the
    /// current phase without re-running auto-search.
    ///
    /// Returns `PhasedCycleResult` — check `needs_user_input` and `done` to
    /// decide what to do next.
    pub async fn cycle(
        &mut self,
        event_tx: Option<&mpsc::Sender<crate::kernel::StreamEvent>>,
    ) -> Result<PhasedCycleResult, KernelError> {
        let mut tools_used: Vec<String> = Vec::new();
        let mut phases_visited: Vec<AgentPhase> = Vec::new();

        // First cycle: auto-search + inject Research prompt
        if !self.auto_search_done {
            self.auto_search_done = true;
            let search_results = self.run_auto_search().await;
            self.messages.push(ChatMessage::system(search_results));
            self.messages.push(ChatMessage::system(RESEARCH_PROMPT.to_string()));
            phases_visited.push(AgentPhase::Research);
            self.send_phase_event(event_tx, AgentPhase::Research).await;
        }

        loop {
            let current_phase = *self.phase.lock().unwrap();

            match current_phase {
                AgentPhase::Research => {
                    let (response_text, used, phase_changed, done) = self
                        .run_phase_loop(event_tx, MAX_RESEARCH_ITERS)
                        .await?;
                    tools_used.extend(used);

                    if phase_changed {
                        let new_phase = *self.phase.lock().unwrap();
                        self.enter_phase(event_tx, &mut phases_visited, new_phase).await;
                        continue;
                    }

                    // LLM responded with text, not tool calls — asking user a question
                    if done && response_text.is_some() {
                        return Ok(PhasedCycleResult {
                            user_response: response_text,
                            done: false,
                            content: None,
                            reasoning_content: None,
                            tools_used,
                            phases_visited,
                            needs_user_input: true,
                        });
                    }

                    // LLM done without text — shouldn't normally happen, but treat as done
                    return Ok(PhasedCycleResult {
                        user_response: None,
                        done: true,
                        content: Some(String::new()),
                        reasoning_content: None,
                        tools_used,
                        phases_visited,
                        needs_user_input: false,
                    });
                }

                AgentPhase::Planning => {
                    let prompt = {
                        let summary = self.context_summary.lock().unwrap().clone()
                            .unwrap_or_else(|| "Research completed.".to_string());
                        PLANNING_PROMPT.replace("{context_summary}", &summary)
                    };
                    self.messages.push(ChatMessage::system(prompt));

                    let (_, used, phase_changed, _) = self
                        .run_phase_loop(event_tx, MAX_PLANNING_ITERS)
                        .await?;
                    tools_used.extend(used);

                    if phase_changed {
                        let new_phase = *self.phase.lock().unwrap();
                        if new_phase == AgentPhase::Execute {
                            self.enter_phase(event_tx, &mut phases_visited, new_phase).await;
                            continue;
                        }
                        // Planning → Done (unlikely but allowed)
                        return Ok(PhasedCycleResult {
                            user_response: None,
                            done: true,
                            content: Some(String::new()),
                            reasoning_content: None,
                            tools_used,
                            phases_visited,
                            needs_user_input: false,
                        });
                    }

                    // Max iters — force to Execute
                    {
                        let mut p = self.phase.lock().unwrap();
                        *p = AgentPhase::Execute;
                    }
                    self.enter_phase(event_tx, &mut phases_visited, AgentPhase::Execute).await;
                    continue;
                }

                AgentPhase::Execute => {
                    self.messages.push(ChatMessage::system(EXECUTE_PROMPT.to_string()));

                    let (response_text, used, phase_changed, done) = self
                        .run_phase_loop(event_tx, u32::MAX)
                        .await?;
                    tools_used.extend(used);

                    if phase_changed {
                        let new_phase = *self.phase.lock().unwrap();
                        if new_phase == AgentPhase::Review {
                            self.enter_phase(event_tx, &mut phases_visited, new_phase).await;
                            continue;
                        }
                        // Execute → Done
                        return Ok(PhasedCycleResult {
                            user_response: None,
                            done: true,
                            content: Some(response_text.unwrap_or_default()),
                            reasoning_content: None,
                            tools_used,
                            phases_visited,
                            needs_user_input: false,
                        });
                    }

                    // Execute completed naturally (LLM responded without tools)
                    {
                        let mut p = self.phase.lock().unwrap();
                        if *p == AgentPhase::Execute {
                            *p = AgentPhase::Done;
                        }
                    }
                    return Ok(PhasedCycleResult {
                        user_response: None,
                        done: true,
                        content: Some(response_text.unwrap_or_default()),
                        reasoning_content: None,
                        tools_used,
                        phases_visited,
                        needs_user_input: false,
                    });
                }

                AgentPhase::Review => {
                    self.messages.push(ChatMessage::system(REVIEW_PROMPT.to_string()));

                    let (response_text, used, phase_changed, _) = self
                        .run_phase_loop(event_tx, MAX_REVIEW_ITERS)
                        .await?;
                    tools_used.extend(used);

                    // Force transition to Done
                    {
                        let mut p = self.phase.lock().unwrap();
                        *p = AgentPhase::Done;
                    }
                    self.send_phase_event(event_tx, AgentPhase::Done).await;
                    phases_visited.push(AgentPhase::Done);

                    return Ok(PhasedCycleResult {
                        user_response: None,
                        done: true,
                        content: Some(response_text.unwrap_or_default()),
                        reasoning_content: None,
                        tools_used,
                        phases_visited,
                        needs_user_input: false,
                    });
                }

                AgentPhase::Done => {
                    return Ok(PhasedCycleResult {
                        user_response: None,
                        done: true,
                        content: Some(String::new()),
                        reasoning_content: None,
                        tools_used,
                        phases_visited,
                        needs_user_input: false,
                    });
                }
            }
        }
    }

    // ── Private helpers ───────────────────────────────────────

    /// Run the SteppableExecutor loop for one phase.
    ///
    /// Returns: (last_response_text, tools_used, phase_changed, done)
    /// - `phase_changed` = true when the LLM called `phase_transition`
    /// - `done` = true when the LLM responded without tool calls (natural end of phase)
    async fn run_phase_loop(
        &mut self,
        event_tx: Option<&mpsc::Sender<crate::kernel::StreamEvent>>,
        max_iters: u32,
    ) -> Result<(Option<String>, Vec<String>, bool, bool), KernelError> {
        let steppable = SteppableExecutor::new(self.ctx.clone());
        let mut ledger = crate::kernel::kernel_executor::TokenLedger::new();
        let mut tools_used: Vec<String> = Vec::new();
        let start_phase = *self.phase.lock().unwrap();

        for iteration in 1..=max_iters {
            debug!("[Phased] iter {} / {} (phase: {:?})", iteration, max_iters, start_phase);

            let result = steppable
                .step(&mut self.messages, &mut ledger, event_tx)
                .await?;

            for tr in &result.tool_results {
                tools_used.push(tr.tool_name.clone());
            }

            // Check if phase_transition was called
            let current_phase = *self.phase.lock().unwrap();
            if current_phase != start_phase {
                let text = result.response.content.clone();
                return Ok((text, tools_used, true, false));
            }

            if !result.should_continue {
                let text = result.response.content.clone();
                return Ok((text, tools_used, false, true));
            }
        }

        info!("[Phased] Max iters ({}) reached for {:?}", max_iters, start_phase);
        Ok((None, tools_used, false, false))
    }

    /// Inject phase entry prompt and notify frontend.
    async fn enter_phase(
        &self,
        event_tx: Option<&mpsc::Sender<crate::kernel::StreamEvent>>,
        phases_visited: &mut Vec<AgentPhase>,
        phase: AgentPhase,
    ) {
        // Prompt injection happens in the phase match arms above (before run_phase_loop).
        // This just handles the event + tracking.
        self.send_phase_event(event_tx, phase).await;
        phases_visited.push(phase);
    }

    /// Run auto-search for the Research phase.
    async fn run_auto_search(&self) -> String {
        let user_query = self.messages
            .iter()
            .rev()
            .find(|m| matches!(m, ChatMessage::User(_)))
            .map(|m| {
                if let ChatMessage::User(content) = m { content.as_str() } else { "" }
            })
            .unwrap_or("");

        if user_query.is_empty() {
            return "[Research Context]\nNo user query found for auto-search.".to_string();
        }

        let ctx = crate::tools::ToolContext::default();

        let wiki_future = async {
            let args = serde_json::json!({"query": user_query, "limit": 5});
            self.ctx.tools.execute("wiki_search", args, &ctx).await
        };

        let history_future = async {
            let args = serde_json::json!({"keywords": user_query, "limit": 10});
            match self.ctx.tools.execute("history_search", args.clone(), &ctx).await {
                Ok(result) => Ok(result),
                Err(_) => self.ctx.tools.execute("query_history", args, &ctx).await,
            }
        };

        let (wiki_result, history_result) = tokio::join!(wiki_future, history_future);

        let wiki_section = match wiki_result {
            Ok(text) if !text.starts_with("No wiki pages") => {
                format!("## Wiki 相关页面\n\n{}", text)
            }
            _ => "## Wiki 相关页面\n\n未找到相关页面。".to_string(),
        };

        let history_section = match history_result {
            Ok(text) if !text.starts_with("No history") => {
                format!("## 历史相关记录\n\n{}", text)
            }
            _ => "## 历史相关记录\n\n未找到相关记录。".to_string(),
        };

        format!(
            "[Research Context — 自动检索]\n\n{}\n\n{}\n\n\
             你可以用 wiki_read 查看完整页面，或 history_search 调整搜索方向。\n\
             需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。",
            wiki_section, history_section
        )
    }

    /// Send a PhaseChanged event to the frontend via the event channel.
    async fn send_phase_event(
        &self,
        event_tx: Option<&mpsc::Sender<crate::kernel::StreamEvent>>,
        phase: AgentPhase,
    ) {
        if let Some(tx) = event_tx {
            let chat_event = gasket_types::ChatEvent::phase_changed(phase);
            let json = serde_json::to_string(&chat_event).unwrap_or_default();
            let _ = tx.send(crate::kernel::StreamEvent::Text {
                content: std::sync::Arc::from(json),
            }).await;
        }
    }
}
```

- [ ] **Step 2: Add module to kernel/mod.rs**

In `gasket/engine/src/kernel/mod.rs`, add to the module declarations:

```rust
pub(crate) mod phased_executor;
```

- [ ] **Step 3: Add re-export to kernel/executor.rs**

In `gasket/engine/src/kernel/executor.rs`, add:

```rust
pub use crate::kernel::phased_executor::{PhasedExecutor, PhasedResult};
```

- [ ] **Step 4: Build to verify**

```bash
cargo build --package gasket-engine
```

Expected: compiles without errors.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased_executor.rs \
        gasket/engine/src/kernel/mod.rs \
        gasket/engine/src/kernel/executor.rs
git commit -m "feat(engine): add PhasedExecutor state machine for Research-Plan-Execute-Review loop"
```

---

### Task 5: KernelConfig integration gate

**Files:**
- Modify: `gasket/engine/src/kernel/context.rs` (add `phased` field)
- Modify: `gasket/engine/src/kernel/mod.rs` (use PhasedExecutor when `phased` is true)

- [ ] **Step 1: Add `phased` field to KernelConfig**

In `gasket/engine/src/kernel/context.rs`, add to the `KernelConfig` struct after `ws_summary_limit`:

```rust
    /// Enable phased execution (Research → Planning → Execute → Review).
    /// When false (default), uses the standard unphased loop.
    pub phased: bool,
```

In `KernelConfig::new()`, add:

```rust
            phased: false,
```

- [ ] **Step 2: Check for compilation errors at other construction sites**

```bash
cargo check --workspace 2>&1 | head -50
```

Add `phased: false` to any `KernelConfig` construction sites that fail.

- [ ] **Step 3: Modify kernel/mod.rs to use PhasedExecutor when `phased` is true**

In `gasket/engine/src/kernel/mod.rs`, modify `execute_streaming`:

```rust
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError> {
    if ctx.config.phased {
        let mut phased = phased_executor::PhasedExecutor::new(ctx.clone(), messages);
        loop {
            let result = phased.cycle(Some(&event_tx)).await?;
            if result.done {
                return Ok(ExecutionResult {
                    content: result.content.unwrap_or_default(),
                    reasoning_content: result.reasoning_content,
                    tools_used: result.tools_used,
                    token_usage: None,
                });
            }
            if result.needs_user_input {
                // Send the LLM's question to the user via event_tx
                if let Some(text) = &result.user_response {
                    let _ = event_tx
                        .send(StreamEvent::content(text.clone()))
                        .await;
                }
                let _ = event_tx.send(StreamEvent::done()).await;
                // The session dispatcher will call execute_streaming again
                // with the user's response appended to messages
                return Err(KernelError::Provider(
                    "PhasedExecutor needs user input — re-invoke with updated messages".to_string(),
                ));
            }
        }
    } else {
        let exec = build_executor(ctx);
        exec.execute_stream_with_options(messages, event_tx, &ExecutorOptions::new())
            .await
    }
}
```

Similarly modify `execute`:

```rust
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError> {
    if ctx.config.phased {
        let mut phased = phased_executor::PhasedExecutor::new(ctx.clone(), messages);
        loop {
            let result = phased.cycle(None).await?;
            if result.done {
                return Ok(ExecutionResult {
                    content: result.content.unwrap_or_default(),
                    reasoning_content: result.reasoning_content,
                    tools_used: result.tools_used,
                    token_usage: None,
                });
            }
            // Non-streaming mode shouldn't normally get needs_user_input
            // (streaming is the primary phased use case); fall through to error
            if result.needs_user_input {
                return Err(KernelError::Provider(
                    "PhasedExecutor needs user input in non-streaming mode".to_string(),
                ));
            }
        }
    } else {
        let exec = build_executor(ctx);
        exec.execute_with_options(messages, &ExecutorOptions::new())
            .await
    }
}
```

Note: The `StreamEvent::Done` sent when `needs_user_input` is true signals to the session dispatcher that the current response is complete but the conversation isn't done. The dispatcher will invoke `execute_streaming` again when the user responds. At that point, the `messages` parameter should include the user's new message appended. The PhasedExecutor will resume from wherever it left off — but wait, this doesn't work because `PhasedExecutor::new()` creates a fresh executor each time. 

**Important design note:** For user clarification to work correctly, the PhasedExecutor must persist across multiple `execute_streaming` calls. This requires the session dispatcher to hold the PhasedExecutor instance, not create a new one each time. The current kernel API (`execute_streaming` as a pure function) doesn't support this.

**Simplification for initial implementation:** Skip the user-clarification re-entrancy for now. The Research phase runs auto-search + sub-loop in one shot. If the LLM wants to ask the user, it calls `phase_transition("execute")` to enter Execute, then asks the question there. This keeps the kernel API unchanged — `execute_streaming` is still a pure function that runs the full phase chain to Done.

This means the `cycle()` loop in Step 3 above simplifies to:

```rust
if ctx.config.phased {
    let mut phased = phased_executor::PhasedExecutor::new(ctx.clone(), messages);
    let result = phased.cycle(Some(&event_tx)).await?;
    // PhasedExecutor always runs to Done (no mid-cycle user I/O yet)
    Ok(ExecutionResult {
        content: result.content.unwrap_or_default(),
        reasoning_content: result.reasoning_content,
        tools_used: result.tools_used,
        token_usage: None,
    })
}
```

The re-entrant design is in the code so it can be activated later when the session dispatcher is updated to hold the executor across calls.

---

### Task 6: Frontend phase indicator

**Files:**
- Modify: `web/src/composables/useChatSession.ts`
- Modify: `web/src/components/ChatMessage.vue` (or the main message panel component)

- [ ] **Step 1: Check current frontend structure**

```bash
ls web/src/components/ | grep -i message
```

- [ ] **Step 2: Add phase tracking to useChatSession**

In `web/src/composables/useChatSession.ts`, add a reactive `currentPhase` ref:

```typescript
// Add near other reactive state declarations
const currentPhase = ref<string | null>(null)

// In the WebSocket message handler, add handling for phase_changed:
case 'phase_changed':
  currentPhase.value = data.phase
  break
```

Export `currentPhase` from the composable's return object.

- [ ] **Step 3: Add phase badge to the chat component**

In the appropriate chat message component, add a phase indicator badge above the message stream:

```vue
<!-- Phase indicator badge -->
<div
  v-if="currentPhase"
  class="phase-badge"
  :class="`phase-${currentPhase}`"
>
  {{ currentPhase }}
</div>
```

Add minimal CSS for the badge:

```css
.phase-badge {
  display: inline-block;
  padding: 2px 10px;
  border-radius: 12px;
  font-size: 12px;
  font-weight: 500;
  margin-bottom: 8px;
  background: var(--color-surface-2);
  color: var(--color-text-secondary);
  transition: all 0.2s ease;
}
```

- [ ] **Step 4: Build frontend**

```bash
cd web && npm run build
```

Expected: no build errors.

- [ ] **Step 5: Commit**

```bash
git add web/src/composables/useChatSession.ts web/src/components/
git commit -m "feat(web): add phase indicator badge for phased agent execution"
```

---

### Task 7: End-to-end verification

**Files:** None new

- [ ] **Step 1: Build entire workspace**

```bash
cargo build --workspace
```

Expected: compiles without errors.

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace
```

Expected: all tests pass.

- [ ] **Step 3: Verify backward compatibility**

Confirm that with `phased: false` (default), the agent loop behaves identically:

```bash
# Run a simple CLI message with default config
cargo run --release --package gasket-cli -- agent -m "hello"
```

Expected: standard agent response, no phase indicators.

- [ ] **Step 4: Manual phased mode test**

Temporarily set `phased: true` in test config and verify phase events are emitted:

```bash
# Enable phased mode and check that phase_changed events appear in stream
```

- [ ] **Step 5: Commit any fixes and finalize**

```bash
git add -A
git commit -m "chore: end-to-end verification of phased agent loop"
```
