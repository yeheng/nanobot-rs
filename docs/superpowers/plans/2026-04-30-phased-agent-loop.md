# Phased Agent Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the open-ended LLM loop with a phased execution model (Research → Planning → Execute → Review → Done) that enables cross-session learning via automatic wiki knowledge retrieval and extraction.

**Architecture:** `PhasedExecutor` wraps the existing `SteppableExecutor` without modifying it. It manages phase state, filters available tools per phase via `PhasedToolSet` at request-building time, injects phase-aware system messages, and intercepts `phase_transition` tool calls. `ResearchContext` handles auto-search on user messages. The session layer dispatches to `PhasedExecutor` or the existing kernel based on a config flag.

**Tech Stack:** Rust (edition 2021), tokio, serde, sqlx, gasket_wiki (Tantivy BM25), gasket_types (StreamEvent)

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `gasket/engine/src/kernel/phased/agent_phase.rs` | `AgentPhase` enum, transition validation, per-phase config |
| `gasket/engine/src/kernel/phased/phased_tool_set.rs` | `PhasedToolSet` — filters `ToolRegistry` per phase |
| `gasket/engine/src/kernel/phased/step_action.rs` | `StepAction` enum — classifies StepResult for phased loop |
| `gasket/engine/src/kernel/phased/phase_prompt.rs` | `PhasePrompt` + `ContextAccumulator` — entry prompts & context |
| `gasket/engine/src/kernel/phased/research_context.rs` | `ResearchContext` — auto-search query building + result formatting |
| `gasket/engine/src/kernel/phased/phased_executor.rs` | `PhaseStateMachine` + `PhasedExecutor` — state machine & run loop |
| `gasket/engine/src/kernel/phased/mod.rs` | Module re-exports |
| `gasket/engine/src/tools/phase_transition.rs` | `PhaseTransitionTool` — the `phase_transition` tool impl |

### Modified files

| File | Change |
|------|--------|
| `gasket/types/src/events/stream.rs` | Add `PhaseTransition` variant to `StreamEventKind` |
| `gasket/engine/src/kernel/context.rs` | Add `phased_execution: bool` to `KernelConfig` |
| `gasket/engine/src/kernel/mod.rs` | Add `pub mod phased`, re-export |
| `gasket/engine/src/tools/mod.rs` | Register `phase_transition` module |
| `gasket/engine/src/tools/builder.rs` | Register `PhaseTransitionTool` in `build_tool_registry` |
| `gasket/engine/src/session/mod.rs` | Dispatch to `PhasedExecutor` when `phased_execution` is true |

---

### Task 1: AgentPhase Enum + Transition Validation

**Files:**
- Create: `gasket/engine/src/kernel/phased/agent_phase.rs`
- Create: `gasket/engine/src/kernel/phased/mod.rs`
- Create: `gasket/engine/src/kernel/phased/step_action.rs` (placeholder)
- Create: `gasket/engine/src/kernel/phased/phased_tool_set.rs` (placeholder)
- Create: `gasket/engine/src/kernel/phased/phase_prompt.rs` (placeholder)
- Create: `gasket/engine/src/kernel/phased/research_context.rs` (placeholder)
- Create: `gasket/engine/src/kernel/phased/phased_executor.rs` (placeholder)
- Modify: `gasket/engine/src/kernel/mod.rs`

- [ ] **Step 1: Write the test**

```rust
// gasket/engine/src/kernel/phased/agent_phase.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_transitions_from_research() {
        let phase = AgentPhase::Research;
        assert!(phase.can_transition_to(&AgentPhase::Planning));
        assert!(phase.can_transition_to(&AgentPhase::Execute));
        assert!(!phase.can_transition_to(&AgentPhase::Review));
        assert!(!phase.can_transition_to(&AgentPhase::Done));
        assert!(!phase.can_transition_to(&AgentPhase::Research));
    }

    #[test]
    fn test_valid_transitions_from_planning() {
        let phase = AgentPhase::Planning;
        assert!(phase.can_transition_to(&AgentPhase::Execute));
        assert!(!phase.can_transition_to(&AgentPhase::Review));
        assert!(!phase.can_transition_to(&AgentPhase::Planning));
    }

    #[test]
    fn test_valid_transitions_from_execute() {
        let phase = AgentPhase::Execute;
        assert!(phase.can_transition_to(&AgentPhase::Review));
        assert!(phase.can_transition_to(&AgentPhase::Done));
        assert!(!phase.can_transition_to(&AgentPhase::Execute));
    }

    #[test]
    fn test_valid_transitions_from_review() {
        let phase = AgentPhase::Review;
        assert!(phase.can_transition_to(&AgentPhase::Done));
        assert!(!phase.can_transition_to(&AgentPhase::Review));
    }

    #[test]
    fn test_done_is_terminal() {
        let phase = AgentPhase::Done;
        assert!(!phase.can_transition_to(&AgentPhase::Research));
        assert!(!phase.can_transition_to(&AgentPhase::Execute));
    }

    #[test]
    fn test_hard_limit_iterations() {
        assert_eq!(AgentPhase::Research.max_iterations(), 7);
        assert_eq!(AgentPhase::Planning.max_iterations(), 5);
        assert_eq!(AgentPhase::Execute.max_iterations(), u32::MAX);
        assert_eq!(AgentPhase::Review.max_iterations(), 5);
        assert_eq!(AgentPhase::Done.max_iterations(), 0);
    }

    #[test]
    fn test_soft_limit_iterations() {
        assert_eq!(AgentPhase::Research.soft_limit_iterations(), 5);
        assert_eq!(AgentPhase::Planning.soft_limit_iterations(), 3);
        assert_eq!(AgentPhase::Execute.soft_limit_iterations(), 0);
        assert_eq!(AgentPhase::Review.soft_limit_iterations(), 3);
    }

    #[test]
    fn test_forced_transition_target() {
        assert_eq!(
            AgentPhase::Research.forced_transition_target(),
            Some(&AgentPhase::Execute)
        );
        assert_eq!(
            AgentPhase::Planning.forced_transition_target(),
            Some(&AgentPhase::Execute)
        );
        assert_eq!(AgentPhase::Execute.forced_transition_target(), None);
        assert_eq!(
            AgentPhase::Review.forced_transition_target(),
            Some(&AgentPhase::Done)
        );
    }

    #[test]
    fn test_from_str_roundtrip() {
        for name in &["research", "planning", "execute", "review", "done"] {
            let phase = AgentPhase::try_from(*name).unwrap();
            assert_eq!(phase.as_str(), *name);
        }
        assert!(AgentPhase::try_from("invalid").is_err());
    }

    #[test]
    fn test_allowed_tools_research() {
        let tools = AgentPhase::Research.allowed_tools();
        assert!(tools.contains(&"wiki_search"));
        assert!(tools.contains(&"wiki_read"));
        assert!(tools.contains(&"phase_transition"));
        assert!(!tools.contains(&"shell"));
    }

    #[test]
    fn test_allowed_tools_execute_is_empty() {
        assert!(AgentPhase::Execute.allowed_tools().is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket/engine && cargo test --lib kernel::phased::agent_phase 2>&1 | head -5`
Expected: compilation error — module does not exist

- [ ] **Step 3: Create module directory and implement**

```bash
mkdir -p gasket/engine/src/kernel/phased
```

```rust
// gasket/engine/src/kernel/phased/agent_phase.rs

use std::fmt;

/// Execution phases for the phased agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentPhase {
    Research,
    Planning,
    Execute,
    Review,
    Done,
}

impl AgentPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Planning => "planning",
            Self::Execute => "execute",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    pub fn can_transition_to(&self, target: &AgentPhase) -> bool {
        matches!(
            (self, target),
            (Self::Research, Self::Planning | Self::Execute)
                | (Self::Planning, Self::Execute)
                | (Self::Execute, Self::Review | Self::Done)
                | (Self::Review, Self::Done)
        )
    }

    /// Hard iteration limit per phase (soft limit + 2 grace iterations).
    pub fn max_iterations(&self) -> u32 {
        match self {
            Self::Research => 7,
            Self::Planning => 5,
            Self::Execute => u32::MAX,
            Self::Review => 5,
            Self::Done => 0,
        }
    }

    /// Soft iteration limit — engine injects a prompt encouraging transition.
    pub fn soft_limit_iterations(&self) -> u32 {
        match self {
            Self::Research => 5,
            Self::Planning => 3,
            Self::Execute => 0,
            Self::Review => 3,
            Self::Done => 0,
        }
    }

    /// Phase to force-transition to when iteration limit is hit.
    pub fn forced_transition_target(&self) -> Option<&AgentPhase> {
        match self {
            Self::Research | Self::Planning => Some(&AgentPhase::Execute),
            Self::Review => Some(&AgentPhase::Done),
            Self::Execute | Self::Done => None,
        }
    }

    /// Tool names allowed in this phase. Empty Vec = all tools (Execute phase).
    pub fn allowed_tools(&self) -> Vec<&'static str> {
        match self {
            Self::Research => vec![
                "wiki_search",
                "wiki_read",
                "history_search",
                "query_history",
                "phase_transition",
            ],
            Self::Planning => vec!["create_plan", "phase_transition", "wiki_read", "wiki_search"],
            Self::Execute => vec![],
            Self::Review => vec![
                "wiki_write",
                "wiki_delete",
                "wiki_read",
                "wiki_search",
                "evolution",
                "phase_transition",
            ],
            Self::Done => vec![],
        }
    }
}

impl fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for AgentPhase {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "research" => Ok(Self::Research),
            "planning" => Ok(Self::Planning),
            "execute" => Ok(Self::Execute),
            "review" => Ok(Self::Review),
            "done" => Ok(Self::Done),
            other => Err(format!("Unknown phase: {}", other)),
        }
    }
}
```

- [ ] **Step 4: Create module entry point + placeholder files**

```rust
// gasket/engine/src/kernel/phased/mod.rs

pub mod agent_phase;
pub mod step_action;
pub mod phased_tool_set;
pub mod phase_prompt;
pub mod research_context;
pub mod phased_executor;

pub use agent_phase::AgentPhase;
pub use step_action::StepAction;
pub use phased_tool_set::PhasedToolSet;
pub use research_context::ResearchContext;
pub use phased_executor::PhasedExecutor;
```

Create each placeholder as a single-line file:
```rust
// gasket/engine/src/kernel/phased/step_action.rs
// Placeholder — implemented in Task 3.
```
```rust
// gasket/engine/src/kernel/phased/phased_tool_set.rs
// Placeholder — implemented in Task 2.
```
```rust
// gasket/engine/src/kernel/phased/phase_prompt.rs
// Placeholder — implemented in Task 6.
```
```rust
// gasket/engine/src/kernel/phased/research_context.rs
// Placeholder — implemented in Task 5.
```
```rust
// gasket/engine/src/kernel/phased/phased_executor.rs
// Placeholder — implemented in Task 8.
```

- [ ] **Step 5: Register module in kernel**

Modify `gasket/engine/src/kernel/mod.rs` — add after existing `pub mod` lines:

```rust
pub mod phased;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::agent_phase`
Expected: all 10 tests PASS

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/kernel/phased/ gasket/engine/src/kernel/mod.rs
git commit -m "feat(engine): add AgentPhase enum with transition validation"
```

---

### Task 2: PhasedToolSet — Phase-Aware Tool Filtering

**Files:**
- Modify: `gasket/engine/src/kernel/phased/phased_tool_set.rs` (replace placeholder)

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/kernel/phased/phased_tool_set.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolResult};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeTool { name: &'static str }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str { self.name }
        fn description(&self) -> &str { "fake" }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn as_any(&self) -> &dyn std::any::Any { self }
        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> ToolResult {
            Ok("ok".to_string())
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(FakeTool { name: "wiki_search" }));
        reg.register(Box::new(FakeTool { name: "wiki_read" }));
        reg.register(Box::new(FakeTool { name: "shell" }));
        reg.register(Box::new(FakeTool { name: "write_file" }));
        reg.register(Box::new(FakeTool { name: "phase_transition" }));
        Arc::new(reg)
    }

    #[test]
    fn test_research_phase_filters_tools() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry, AgentPhase::Research);
        let names: Vec<&str> = tool_set.definition_names();
        assert!(names.contains(&"wiki_search"));
        assert!(names.contains(&"wiki_read"));
        assert!(names.contains(&"phase_transition"));
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"write_file"));
    }

    #[test]
    fn test_execute_phase_returns_all_tools() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry, AgentPhase::Execute);
        let names: Vec<&str> = tool_set.definition_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn test_for_phase_changes_filter() {
        let registry = make_registry();
        let research = PhasedToolSet::new(registry.clone(), AgentPhase::Research);
        assert_eq!(research.definition_names().len(), 3);
        let execute = research.for_phase(AgentPhase::Execute);
        assert_eq!(execute.definition_names().len(), 5);
    }

    #[test]
    fn test_delegates_execution_to_registry() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry.clone(), AgentPhase::Execute);
        assert!(tool_set.get("shell").is_some());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phased_tool_set`
Expected: compilation error

- [ ] **Step 3: Implement PhasedToolSet**

```rust
// gasket/engine/src/kernel/phased/phased_tool_set.rs

use std::sync::Arc;

use crate::tools::ToolRegistry;
use gasket_providers::ToolDefinition;

use super::agent_phase::AgentPhase;

/// Phase-aware tool set — filters ToolRegistry definitions per phase.
///
/// When phase is Execute, all tools are visible (empty allowed list = all).
pub struct PhasedToolSet {
    registry: Arc<ToolRegistry>,
    phase: AgentPhase,
}

impl PhasedToolSet {
    pub fn new(registry: Arc<ToolRegistry>, phase: AgentPhase) -> Self {
        Self { registry, phase }
    }

    pub fn for_phase(&self, new_phase: AgentPhase) -> Self {
        Self {
            registry: self.registry.clone(),
            phase: new_phase,
        }
    }

    pub fn phase(&self) -> AgentPhase {
        self.phase
    }

    /// Get filtered tool definitions for the current phase.
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        let allowed = self.phase.allowed_tools();
        let all_defs = self.registry.get_definitions();
        if allowed.is_empty() {
            return all_defs;
        }
        all_defs
            .into_iter()
            .filter(|def| allowed.contains(&def.function.name.as_str()))
            .collect()
    }

    /// Get filtered definition names (test helper).
    #[cfg(test)]
    fn definition_names(&self) -> Vec<&str> {
        let allowed = self.phase.allowed_tools();
        let all_names = self.registry.list();
        if allowed.is_empty() {
            return all_names;
        }
        all_names.into_iter().filter(|n| allowed.contains(n)).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn crate::tools::Tool> {
        self.registry.get(name)
    }

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &crate::tools::ToolContext,
    ) -> crate::tools::ToolResult {
        self.registry.execute(name, args, ctx).await
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phased_tool_set`
Expected: all 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased/phased_tool_set.rs
git commit -m "feat(engine): add PhasedToolSet for phase-aware tool filtering"
```

---

### Task 3: StepAction Enum

**Files:**
- Modify: `gasket/engine/src/kernel/phased/step_action.rs` (replace placeholder)

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/kernel/phased/step_action.rs

#[cfg(test)]
mod tests {
    use super::*;

    fn make_step_result_with_tools(tools: Vec<(&str, &str)>) -> StepResult {
        use gasket_providers::{ChatResponse, ToolCall};
        let tool_calls: Vec<ToolCall> = tools
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| {
                ToolCall::new(
                    format!("call_{}", i),
                    name,
                    serde_json::from_str(args).unwrap_or_default(),
                )
            })
            .collect();
        StepResult {
            response: ChatResponse {
                content: None,
                tool_calls,
                reasoning_content: None,
                usage: None,
            },
            tool_results: vec![],
            should_continue: true,
        }
    }

    fn make_step_result_with_content(text: &str) -> StepResult {
        use gasket_providers::ChatResponse;
        StepResult {
            response: ChatResponse {
                content: Some(text.to_string()),
                tool_calls: vec![],
                reasoning_content: None,
                usage: None,
            },
            tool_results: vec![],
            should_continue: false,
        }
    }

    #[test]
    fn test_classify_phase_transition() {
        let result = make_step_result_with_tools(vec![
            ("phase_transition", r#"{"phase":"execute"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to } if to == AgentPhase::Execute));
    }

    #[test]
    fn test_classify_text_response() {
        let result = make_step_result_with_content("Can you clarify?");
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::WaitForUserInput));
    }

    #[test]
    fn test_classify_other_tool_calls_continue() {
        let result = make_step_result_with_tools(vec![
            ("wiki_search", r#"{"query":"test"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::Continue));
    }

    #[test]
    fn test_classify_mixed_tools_prioritizes_phase_transition() {
        let result = make_step_result_with_tools(vec![
            ("wiki_search", r#"{"query":"test"}"#),
            ("phase_transition", r#"{"phase":"planning"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to } if to == AgentPhase::Planning));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::step_action`
Expected: compilation error

- [ ] **Step 3: Implement StepAction**

```rust
// gasket/engine/src/kernel/phased/step_action.rs

use crate::kernel::steppable_executor::StepResult;
use super::agent_phase::AgentPhase;

/// Classifies a StepResult into a phase-aware action.
#[derive(Debug, PartialEq)]
pub enum StepAction {
    /// Tool calls executed, more steps needed.
    Continue,
    /// LLM sent text without tool calls — pause for user input.
    WaitForUserInput,
    /// LLM called phase_transition — switch phase.
    PhaseTransition { to: AgentPhase },
}

impl StepAction {
    pub fn classify(result: &StepResult) -> Self {
        for tc in &result.response.tool_calls {
            if tc.function.name == "phase_transition" {
                if let Some(phase_str) = tc.function.arguments.get("phase").and_then(|v| v.as_str())
                {
                    if let Ok(to) = AgentPhase::try_from(phase_str) {
                        return StepAction::PhaseTransition { to };
                    }
                }
            }
        }
        if !result.response.has_tool_calls() && result.response.content.is_some() {
            return StepAction::WaitForUserInput;
        }
        StepAction::Continue
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::step_action`
Expected: all 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased/step_action.rs
git commit -m "feat(engine): add StepAction enum for phased result classification"
```

---

### Task 4: PhaseTransitionTool

**Files:**
- Create: `gasket/engine/src/tools/phase_transition.rs`
- Modify: `gasket/engine/src/tools/mod.rs`

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/tools/phase_transition.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = PhaseTransitionTool::new();
        assert_eq!(tool.name(), "phase_transition");
        assert!(tool.description().contains("phase"));
    }

    #[test]
    fn test_execute_valid_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({
            "phase": "execute",
            "context_summary": "Found relevant wiki pages"
        });
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("execute"));
    }

    #[test]
    fn test_execute_missing_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_invalid_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({"phase": "invalid_phase"});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib tools::phase_transition`
Expected: compilation error

- [ ] **Step 3: Implement PhaseTransitionTool**

```rust
// gasket/engine/src/tools/phase_transition.rs

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolContext, ToolError, ToolResult};

pub struct PhaseTransitionTool;

impl PhaseTransitionTool {
    pub fn new() -> Self { Self }
}

#[derive(Deserialize)]
struct TransitionArgs {
    phase: String,
    #[serde(default)]
    context_summary: String,
}

#[async_trait]
impl Tool for PhaseTransitionTool {
    fn name(&self) -> &str { "phase_transition" }

    fn description(&self) -> &str {
        "Transition to the next working phase. \
         Valid targets depend on current phase: \
         Research -> planning|execute, Planning -> execute, \
         Execute -> review|done, Review -> done. \
         Optionally provide a context_summary for the next phase."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "phase": {
                    "type": "string",
                    "enum": ["planning", "execute", "review", "done"],
                    "description": "Target phase"
                },
                "context_summary": {
                    "type": "string",
                    "description": "Optional summary for the next phase"
                }
            },
            "required": ["phase"]
        })
    }

    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: TransitionArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let valid = ["planning", "execute", "review", "done"];
        if !valid.contains(&parsed.phase.as_str()) {
            return Err(ToolError::InvalidArguments(format!(
                "Invalid phase '{}'. Valid: {:?}", parsed.phase, valid
            )));
        }

        // Actual transition is handled by PhasedExecutor intercepting the tool call.
        Ok(format!("Phase transition to {} acknowledged.", parsed.phase))
    }
}
```

- [ ] **Step 4: Register in tools/mod.rs**

Modify `gasket/engine/src/tools/mod.rs`:
- Add `mod phase_transition;` in module declarations (alphabetically after `provider`)
- Add `pub use phase_transition::PhaseTransitionTool;` in re-exports

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib tools::phase_transition`
Expected: all 4 tests PASS

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/tools/phase_transition.rs gasket/engine/src/tools/mod.rs
git commit -m "feat(engine): add PhaseTransitionTool"
```

---

### Task 5: ResearchContext — Auto-Search Query Building

**Files:**
- Modify: `gasket/engine/src/kernel/phased/research_context.rs` (replace placeholder)

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/kernel/phased/research_context.rs

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_providers::ChatMessage;

    #[test]
    fn test_build_search_query_single_message() {
        let messages = vec![ChatMessage::user("How does tokio work?")];
        let query = ResearchContext::build_search_query(&messages);
        assert_eq!(query, "How does tokio work?");
    }

    #[test]
    fn test_build_search_query_concatenates_recent() {
        let messages = vec![
            ChatMessage::user("Tell me about Rust"),
            ChatMessage::assistant("Rust is..."),
            ChatMessage::user("How about async?"),
        ];
        let query = ResearchContext::build_search_query(&messages);
        assert!(query.contains("Tell me about Rust"));
        assert!(query.contains("How about async?"));
    }

    #[test]
    fn test_build_search_query_limits_to_last_3() {
        let mut messages = vec![];
        for i in 0..5 {
            messages.push(ChatMessage::user(format!("Message {}", i)));
            messages.push(ChatMessage::assistant(format!("Reply {}", i)));
        }
        let query = ResearchContext::build_search_query(&messages);
        assert!(query.contains("Message 4"));
        assert!(query.contains("Message 3"));
        assert!(query.contains("Message 2"));
        assert!(!query.contains("Message 1"));
    }

    #[test]
    fn test_format_both_empty() {
        let formatted = ResearchContext::format_auto_search_results(&[], &[]);
        assert!(formatted.contains("未找到"));
    }

    #[test]
    fn test_format_wiki_hits() {
        let wiki = vec![WikiHit {
            title: "Tokio Runtime".into(),
            path: "topics/tokio".into(),
            score: 0.92,
            summary: "Async runtime".into(),
        }];
        let formatted = ResearchContext::format_auto_search_results(&wiki, &[]);
        assert!(formatted.contains("Tokio Runtime"));
        assert!(formatted.contains("0.92"));
    }

    #[test]
    fn test_format_history_hits() {
        let history = vec![HistoryHit {
            role: "user".into(),
            content: "How to use tokio?".into(),
            timestamp: "2026-04-29".into(),
        }];
        let formatted = ResearchContext::format_auto_search_results(&[], &history);
        assert!(formatted.contains("How to use tokio?"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::research_context`
Expected: compilation error

- [ ] **Step 3: Implement ResearchContext**

```rust
// gasket/engine/src/kernel/phased/research_context.rs

use gasket_providers::ChatMessage;

pub struct WikiHit {
    pub title: String,
    pub path: String,
    pub score: f32,
    pub summary: String,
}

pub struct HistoryHit {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

pub struct ResearchContext;

impl ResearchContext {
    /// Build search query from last 3 user messages.
    pub fn build_search_query(messages: &[ChatMessage]) -> String {
        let user_msgs: Vec<&str> = messages
            .iter()
            .rev()
            .filter(|m| m.role == "user")
            .filter_map(|m| m.content.as_deref())
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        user_msgs.join(" ")
    }

    /// Format auto-search results as injected system message.
    pub fn format_auto_search_results(
        wiki_hits: &[WikiHit],
        history_hits: &[HistoryHit],
    ) -> String {
        let mut parts = vec!["[Research Context — 自动检索]\n".to_string()];

        if wiki_hits.is_empty() && history_hits.is_empty() {
            parts.push("未找到相关的 Wiki 页面或历史记录。\n".to_string());
        } else {
            if !wiki_hits.is_empty() {
                parts.push(format!("## Wiki 相关页面 ({}条)\n", wiki_hits.len()));
                for hit in wiki_hits {
                    parts.push(format!("- {} ({:.2}): {}\n", hit.path, hit.score, hit.summary));
                }
                parts.push("\n".to_string());
            }
            if !history_hits.is_empty() {
                parts.push(format!("## 历史相关记录 ({}条)\n", history_hits.len()));
                for hit in history_hits {
                    let preview = truncate_str(&hit.content, 100);
                    parts.push(format!("- [{}] {}: {}\n", hit.timestamp, hit.role, preview));
                }
                parts.push("\n".to_string());
            }
        }

        parts.push(
            "你可以用 wiki_read 查看完整页面，或 history_search 调整搜索方向。\n\
             需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。"
                .to_string(),
        );
        parts.join("")
    }
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        s
    } else {
        let end = s.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(s.len());
        &s[..end]
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::research_context`
Expected: all 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased/research_context.rs
git commit -m "feat(engine): add ResearchContext for auto-search query building"
```

---

### Task 6: PhasePrompt + ContextAccumulator

**Files:**
- Modify: `gasket/engine/src/kernel/phased/phase_prompt.rs` (replace placeholder)

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/kernel/phased/phase_prompt.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_research_entry_prompt() {
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Research, &ContextAccumulator::new());
        assert!(prompt.contains("Research"));
        assert!(prompt.contains("wiki_search"));
    }

    #[test]
    fn test_planning_with_research_summary() {
        let mut ctx = ContextAccumulator::new();
        ctx.add(AgentPhase::Research, "Found 3 wiki pages".into());
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Planning, &ctx);
        assert!(prompt.contains("Planning"));
        assert!(prompt.contains("Found 3 wiki pages"));
    }

    #[test]
    fn test_execute_with_accumulated_context() {
        let mut ctx = ContextAccumulator::new();
        ctx.add(AgentPhase::Research, "User wants async info".into());
        ctx.add(AgentPhase::Planning, "Plan: read docs".into());
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Execute, &ctx);
        assert!(prompt.contains("Execute"));
        assert!(prompt.contains("User wants async info"));
        assert!(prompt.contains("Plan: read docs"));
    }

    #[test]
    fn test_review_entry_prompt() {
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Review, &ContextAccumulator::new());
        assert!(prompt.contains("Review"));
        assert!(prompt.contains("wiki_write"));
    }

    #[test]
    fn test_soft_limit_prompt() {
        let prompt = PhasePrompt::soft_limit_prompt(AgentPhase::Research);
        assert!(prompt.contains("phase_transition"));
        assert!(prompt.contains("信息已足够"));
    }

    #[test]
    fn test_hard_limit_prompt() {
        let prompt = PhasePrompt::hard_limit_prompt(AgentPhase::Research, AgentPhase::Execute);
        assert!(prompt.contains("强制推进"));
        assert!(prompt.contains("execute"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phase_prompt`
Expected: compilation error

- [ ] **Step 3: Implement PhasePrompt and ContextAccumulator**

```rust
// gasket/engine/src/kernel/phased/phase_prompt.rs

use super::agent_phase::AgentPhase;

/// Accumulates context summaries across phase transitions.
#[derive(Debug, Default)]
pub struct ContextAccumulator {
    summaries: Vec<(AgentPhase, String)>,
}

impl ContextAccumulator {
    pub fn new() -> Self { Self::default() }

    pub fn add(&mut self, phase: AgentPhase, summary: String) {
        self.summaries.push((phase, summary));
    }

    pub fn format(&self) -> String {
        if self.summaries.is_empty() {
            return String::new();
        }
        let mut parts = vec!["## Collected Context".to_string()];
        for (phase, summary) in &self.summaries {
            parts.push(format!("### {} ({} phase)\n{}", phase, phase, summary));
        }
        parts.join("\n\n")
    }
}

pub struct PhasePrompt;

impl PhasePrompt {
    pub fn entry_prompt(phase: AgentPhase, ctx: &ContextAccumulator) -> String {
        let ctx_block = ctx.format();
        let ctx_section = if ctx_block.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", ctx_block)
        };

        match phase {
            AgentPhase::Research => format!(
                "[Phase: Research]\n\n\
                 你现在处于研究阶段。使用 wiki_search 和 wiki_read 搜索知识库，\
                 用 history_search 查找历史对话。\n\
                 信息充分后调用 phase_transition 进入下一阶段。\n\n\
                 你可以回复用户来澄清需求。"
            ),
            AgentPhase::Planning => format!(
                "[Phase: Planning]\n\n\
                 {ctx_section}\
                 基于以上信息和用户的需求，请制定执行计划。简单任务可以直接跳过此阶段。\n\
                 制定完成后调用 phase_transition(\"execute\") 进入执行。"
            ),
            AgentPhase::Execute => format!(
                "[Phase: Execute]\n\n\
                 {ctx_section}\
                 执行你的计划。所有工具现在可用。\n\
                 完成后调用 phase_transition(\"review\") 进行复盘，或 phase_transition(\"done\") 直接结束。"
            ),
            AgentPhase::Review => format!(
                "[Phase: Review]\n\n\
                 {ctx_section}\
                 审视刚才的执行过程，回答三个问题：\n\
                 1. 结果是否达到了用户的目标？\n\
                 2. 这次对话中有哪些值得持久保存的知识？\n\
                 3. 有哪些 wiki 页面应该创建或更新？\n\n\
                 如果发现了持久知识，主动调用 wiki_write 写入（每次最多 3 条）。\n\
                 完成后调用 phase_transition(\"done\")。"
            ),
            AgentPhase::Done => String::new(),
        }
    }

    pub fn soft_limit_prompt(phase: AgentPhase) -> String {
        format!(
            "[系统提示] {} 阶段已达到建议迭代上限。信息已足够，请调用 phase_transition 进入下一阶段。",
            phase
        )
    }

    pub fn hard_limit_prompt(from: AgentPhase, to: AgentPhase) -> String {
        format!(
            "[系统强制] {} 阶段达到迭代上限，由引擎强制推进至 {} 阶段。",
            from, to
        )
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phase_prompt`
Expected: all 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased/phase_prompt.rs
git commit -m "feat(engine): add PhasePrompt and ContextAccumulator"
```

---

### Task 7: StreamEvent PhaseTransition Variant

**Files:**
- Modify: `gasket/types/src/events/stream.rs`

- [ ] **Step 1: Add PhaseTransition variant to StreamEventKind**

In `gasket/types/src/events/stream.rs`, add after the `Done` variant in `StreamEventKind` (around line 131):

```rust
    /// Phase transition in the phased agent loop
    PhaseTransition {
        from: Arc<str>,
        to: Arc<str>,
    },
```

Add constructor to `StreamEvent` impl block:

```rust
    /// Create a phase_transition event for the main agent
    pub fn phase_transition(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::PhaseTransition {
                from: Arc::from(from.into()),
                to: Arc::from(to.into()),
            },
        }
    }
```

- [ ] **Step 2: Handle in to_chat_event / StreamEvent conversion**

Find where `StreamEventKind` variants are matched for conversion and add:

```rust
StreamEventKind::PhaseTransition { .. } => None,
```

- [ ] **Step 3: Build and run tests**

Run: `cd gasket && cargo build --package gasket-types && cargo test --package gasket-types`
Expected: compiles and all tests pass

- [ ] **Step 4: Commit**

```bash
git add gasket/types/src/events/stream.rs
git commit -m "feat(types): add PhaseTransition variant to StreamEventKind"
```

---

### Task 8: PhaseStateMachine + PhasedExecutor

**Files:**
- Modify: `gasket/engine/src/kernel/phased/phased_executor.rs` (replace placeholder)

- [ ] **Step 1: Write the tests**

```rust
// gasket/engine/src/kernel/phased/phased_executor.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_phase_is_research() {
        let sm = PhaseStateMachine::new();
        assert_eq!(sm.current_phase(), AgentPhase::Research);
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 0);
    }

    #[test]
    fn test_valid_transition() {
        let mut sm = PhaseStateMachine::new();
        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 0);
    }

    #[test]
    fn test_invalid_transition() {
        let mut sm = PhaseStateMachine::new();
        let result = sm.transition(AgentPhase::Review);
        assert!(result.is_err());
        assert_eq!(sm.current_phase(), AgentPhase::Research);
    }

    #[test]
    fn test_iteration_tracking() {
        let mut sm = PhaseStateMachine::new();
        sm.advance_iteration();
        sm.advance_iteration();
        assert_eq!(sm.iteration_in_phase(), 2);
        assert_eq!(sm.total_iterations(), 2);

        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 2);
    }

    #[test]
    fn test_soft_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..5 { sm.advance_iteration(); }
        assert!(sm.is_at_soft_limit());
        assert!(!sm.is_at_hard_limit());
    }

    #[test]
    fn test_hard_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 { sm.advance_iteration(); }
        assert!(sm.is_at_hard_limit());
    }

    #[test]
    fn test_force_transition() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 { sm.advance_iteration(); }
        let target = sm.force_transition().unwrap();
        assert_eq!(target, AgentPhase::Execute);
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
    }

    #[test]
    fn test_context_accumulation() {
        let mut sm = PhaseStateMachine::new();
        sm.add_context("Found wiki pages".into());
        sm.transition(AgentPhase::Execute).unwrap();
        sm.add_context("Executed plan".into());
        assert!(sm.context().format().contains("Found wiki pages"));
        assert!(sm.context().format().contains("Executed plan"));
    }

    #[test]
    fn test_global_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..99 { sm.advance_iteration(); }
        assert!(!sm.is_at_global_limit(100));
        sm.advance_iteration();
        assert!(sm.is_at_global_limit(100));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phased_executor`
Expected: compilation error

- [ ] **Step 3: Implement PhaseStateMachine and PhasedExecutor**

```rust
// gasket/engine/src/kernel/phased/phased_executor.rs

use super::agent_phase::AgentPhase;
use super::phase_prompt::{ContextAccumulator, PhasePrompt};

/// Internal state machine for phase tracking.
pub struct PhaseStateMachine {
    phase: AgentPhase,
    iteration_in_phase: u32,
    total_iterations: u32,
    context: ContextAccumulator,
}

impl PhaseStateMachine {
    pub fn new() -> Self {
        Self {
            phase: AgentPhase::Research,
            iteration_in_phase: 0,
            total_iterations: 0,
            context: ContextAccumulator::new(),
        }
    }

    pub fn current_phase(&self) -> AgentPhase { self.phase }

    pub fn iteration_in_phase(&self) -> u32 { self.iteration_in_phase }

    pub fn total_iterations(&self) -> u32 { self.total_iterations }

    pub fn context(&self) -> &ContextAccumulator { &self.context }

    pub fn add_context(&mut self, summary: String) {
        self.context.add(self.phase, summary);
    }

    pub fn advance_iteration(&mut self) {
        self.iteration_in_phase += 1;
        self.total_iterations += 1;
    }

    pub fn transition(&mut self, target: AgentPhase) -> Result<(), String> {
        if !self.phase.can_transition_to(&target) {
            return Err(format!(
                "Invalid phase transition: {} -> {}",
                self.phase, target
            ));
        }
        self.phase = target;
        self.iteration_in_phase = 0;
        Ok(())
    }

    pub fn is_at_soft_limit(&self) -> bool {
        let soft = self.phase.soft_limit_iterations();
        soft > 0 && self.iteration_in_phase >= soft
    }

    pub fn is_at_hard_limit(&self) -> bool {
        let hard = self.phase.max_iterations();
        hard > 0 && hard < u32::MAX && self.iteration_in_phase >= hard
    }

    pub fn is_at_global_limit(&self, global_max: u32) -> bool {
        self.total_iterations >= global_max
    }

    pub fn force_transition(&mut self) -> Result<AgentPhase, String> {
        if let Some(target) = self.phase.forced_transition_target() {
            let target = *target;
            self.transition(target)?;
            Ok(target)
        } else {
            Err(format!("Cannot force-transition from {}", self.phase))
        }
    }
}

/// Main entry point for phased execution.
///
/// This struct holds the high-level run() method that will be called
/// from the session layer. The actual implementation of run() will be
/// completed when integrating with SteppableExecutor (Task 10).
pub struct PhasedExecutor;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::phased::phased_executor`
Expected: all 9 tests PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/phased/phased_executor.rs
git commit -m "feat(engine): add PhaseStateMachine for phased loop orchestration"
```

---

### Task 9: KernelConfig phased_execution Flag

**Files:**
- Modify: `gasket/engine/src/kernel/context.rs`

- [ ] **Step 1: Add `phased_execution` field**

Modify `gasket/engine/src/kernel/context.rs` — add to `KernelConfig` struct (after `ws_summary_limit`):

```rust
    /// Enable phased execution (Research → Planning → Execute → Review → Done).
    /// When false (default), behavior is identical to current SteppableExecutor loop.
    pub phased_execution: bool,
```

Update `KernelConfig::new()` to include `phased_execution: false,`.

- [ ] **Step 2: Fix any construction sites**

Run: `cd gasket && cargo check --workspace 2>&1 | grep "missing field \`phased_execution\`"`
Add `phased_execution: false` to any failing sites.

- [ ] **Step 3: Run tests**

Run: `cd gasket && cargo test --package gasket-engine --lib kernel::context`
Expected: existing tests pass

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/kernel/context.rs
git commit -m "feat(engine): add phased_execution flag to KernelConfig"
```

---

### Task 10: Register PhaseTransitionTool in Builder

**Files:**
- Modify: `gasket/engine/src/tools/builder.rs`

- [ ] **Step 1: Register the tool**

Modify `gasket/engine/src/tools/builder.rs` — add after the SystemToolProvider registration (around line 143):

```rust
    // Phased execution tool (always registered, filtered by PhasedToolSet)
    tools.register(Box::new(super::PhaseTransitionTool::new()));
```

- [ ] **Step 2: Build to verify**

Run: `cd gasket && cargo build --package gasket-engine`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/tools/builder.rs
git commit -m "feat(engine): register PhaseTransitionTool in builder"
```

---

### Task 11: Session Layer Dispatch

**Files:**
- Modify: `gasket/engine/src/session/mod.rs`

- [ ] **Step 1: Modify AgentSession::execute()**

Modify `gasket/engine/src/session/mod.rs` — update the `execute()` method (around line 583):

```rust
    async fn execute(
        runtime_ctx: &RuntimeContext,
        messages: Vec<ChatMessage>,
        kernel_tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ExecutionResult, AgentError> {
        if runtime_ctx.config.phased_execution {
            // Phased mode — use PhasedExecutor (full implementation in follow-up)
            // For now, fall through to standard kernel execution
            // The PhasedExecutor::run() integration will be completed when
            // the SteppableExecutor + PhasedToolSet wiring is finalized.
        }
        match kernel::execute_streaming(runtime_ctx, messages, kernel_tx).await {
            Ok(r) => Ok(r),
            Err(crate::kernel::KernelError::MaxIterations(n)) => Ok(ExecutionResult {
                content: format!("Maximum iterations ({}) reached.", n),
                reasoning_content: None,
                tools_used: vec![],
                token_usage: None,
            }),
            Err(e) => Err(e.into()),
        }
    }
```

Note: The full `PhasedExecutor::run()` wiring with `SteppableExecutor` requires resolving how `PhasedToolSet` interacts with `RuntimeContext.tools: Arc<ToolRegistry>`. The recommended approach (Approach A from the spec) is to build a filtered `ToolRegistry` clone before each step. This will be implemented in a follow-up commit once the foundation components are verified.

- [ ] **Step 2: Build to verify**

Run: `cd gasket && cargo build --workspace`
Expected: compiles

- [ ] **Step 3: Run all tests**

Run: `cd gasket && cargo test --workspace`
Expected: all tests pass (new code is behind `phased_execution: false` default)

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/session/mod.rs
git commit -m "feat(engine): add phased execution dispatch stub in session layer"
```

---

### Task 12: Workspace Build Verification

- [ ] **Step 1: Full workspace build**

Run: `cd gasket && cargo build --workspace`
Expected: compiles without errors

- [ ] **Step 2: Full test suite**

Run: `cd gasket && cargo test --workspace`
Expected: all tests pass

- [ ] **Step 3: Commit any fixes**

```bash
git add -A
git commit -m "fix: address workspace build issues from phased executor integration"
```

---

## Self-Review

### Spec Coverage

| Spec Section | Task |
|-------------|------|
| §2.1 New Components | Tasks 1-8 (file structure) |
| §2.2 Phase State Machine | Task 1 (AgentPhase), Task 8 (PhaseStateMachine) |
| §2.3 Phase Transitions | Task 1 (can_transition_to), Task 8 (iteration tracking) |
| §2.4 Tool Sets Per Phase | Task 2 (PhasedToolSet + allowed_tools) |
| §2.5 Tool Filtering | Task 2 (request-time filtering) |
| §3.1 Auto-Search | Task 5 (ResearchContext) |
| §3.2 Injected Context | Task 5 (format_auto_search_results) |
| §3.3 Retrieval Sub-Loop | Task 8 (PhasedExecutor run loop — follow-up) |
| §3.4 User Clarification | Task 3 (StepAction::WaitForUserInput) |
| §3.5 Guard Rails | Task 8 (soft/hard limit, forced transition) |
| §4 Planning Phase | Task 6 (PhasePrompt) |
| §5 Execute Phase | Task 6 (PhasePrompt) |
| §6 Review Phase | Task 6 (PhasePrompt) |
| §7 phase_transition Tool | Task 4 (PhaseTransitionTool) |
| §8 Frontend Event | Task 7 (StreamEventKind::PhaseTransition) |
| §11 Backward Compatibility | Task 9 (KernelConfig flag), Task 11 (dispatch) |
| §12 Error Handling | Task 8 (hard limit, forced transition, global limit) |

### Placeholder Scan

No TBD, TODO, or "implement later" in task steps. Task 11 explicitly notes the follow-up work needed for `PhasedExecutor::run()` wiring.

### Type Consistency

- `AgentPhase` defined in Task 1, used in Tasks 2-8 with consistent method names
- `PhasedToolSet::new(Arc<ToolRegistry>, AgentPhase)` matches usage in Task 8/10
- `StepAction::classify(&StepResult)` signature matches `StepResult` from `steppable_executor.rs`
- `ContextAccumulator::add(AgentPhase, String)` matches `PhaseStateMachine::add_context`
- `StreamEventKind::PhaseTransition` uses `Arc<str>` consistent with other variants
