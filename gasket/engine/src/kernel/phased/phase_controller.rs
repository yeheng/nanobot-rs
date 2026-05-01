//! Phase controller — strategy object injected into the unified `run_loop`.
//!
//! Encapsulates all phased-execution logic so `kernel_executor::run_loop`
//! remains a single loop with optional phase hooks.

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::kernel::context::RuntimeContext;
use crate::kernel::steppable_executor::StepResult;
use crate::kernel::stream::StreamEvent;
use crate::kernel::tool_executor::ToolCallResult;
use crate::tools::ToolContext;

use gasket_providers::ChatMessage;
use tokio::sync::mpsc;

use super::agent_phase::{AgentPhase, PhaseContext, PhaseDefinition, default_definitions};
use super::phase_prompt::{ContextAccumulator, PhasePrompt};
use super::research_context::ResearchContext;
use super::step_action::StepAction;
use gasket_providers::MessageRole;

// ── State machine ──────────────────────────────────────────────────

pub struct PhaseStateMachine {
    phase: AgentPhase,
    iteration_in_phase: u32,
    total_iterations: u32,
    context: ContextAccumulator,
}

impl PhaseStateMachine {
    pub fn starting_at(phase: AgentPhase) -> Self {
        Self {
            phase,
            iteration_in_phase: 0,
            total_iterations: 0,
            context: ContextAccumulator::new(),
        }
    }

    pub fn current_phase(&self) -> AgentPhase {
        self.phase
    }
    pub fn context(&self) -> &ContextAccumulator {
        &self.context
    }

    pub fn add_context(&mut self, summary: String) {
        self.context.add(self.phase, summary);
    }

    pub fn advance_iteration(&mut self) {
        self.iteration_in_phase += 1;
        self.total_iterations += 1;
    }

    /// Transition using PhaseDefinition's allowed_transitions.
    pub fn transition_def(&mut self, def: &PhaseDefinition, target: AgentPhase) -> Result<(), String> {
        if !def.allowed_transitions.contains(&target) {
            return Err(format!(
                "Invalid phase transition: {} -> {}",
                self.phase, target
            ));
        }
        self.phase = target;
        self.iteration_in_phase = 0;
        Ok(())
    }

    pub fn is_at_soft_limit(&self, def: &PhaseDefinition) -> bool {
        let soft = def.soft_limit_iterations;
        soft > 0 && self.iteration_in_phase >= soft
    }

    pub fn is_at_hard_limit(&self, def: &PhaseDefinition) -> bool {
        let hard = def.max_iterations;
        hard > 0 && hard < u32::MAX && self.iteration_in_phase >= hard
    }

    pub fn is_at_global_limit(&self, global_max: u32) -> bool {
        self.total_iterations >= global_max
    }

    pub fn force_transition(&mut self, def: &PhaseDefinition) -> Result<AgentPhase, String> {
        if let Some(target) = def.forced_transition_target {
            self.transition_def(def, target)?;
            Ok(target)
        } else {
            Err(format!("Cannot force-transition from {}", self.phase))
        }
    }
}

// ── Controller ─────────────────────────────────────────────────────

/// Action returned by `post_step()` for the loop to execute.
pub enum PhaseAction {
    /// Normal tool execution — loop continues.
    Continue,
    /// Phase changed — messages already truncated + new prompt injected.
    Transition,
    /// LLM sent text without tools — loop should break.
    /// Contains `interrupted_phase: Option<String>`.
    Interrupt(Option<String>),
    /// Transition rejected — hard gate failed or invalid target.
    /// Contains reason to inject as system message so LLM can self-correct.
    Reject(String),
}

/// Encapsulates all phased-execution logic as a strategy for `run_loop`.
pub struct PhaseController {
    state: PhaseStateMachine,
    ctx: RuntimeContext,
    /// Self-contained phase definitions (entry prompt, tools, limits, gates, checklist).
    definitions: std::collections::HashMap<AgentPhase, PhaseDefinition>,
    /// Execution trace for the current phase — feeds gate/checklist verification.
    phase_trace: PhaseContext,
}

impl PhaseController {
    pub fn new(ctx: &RuntimeContext, start_phase: Option<AgentPhase>) -> Self {
        let initial = start_phase.unwrap_or(AgentPhase::Research);
        Self {
            state: PhaseStateMachine::starting_at(initial),
            ctx: ctx.clone(),
            definitions: default_definitions(),
            phase_trace: PhaseContext::new(),
        }
    }

    /// One-time init: auto-search + entry prompt + initial event.
    ///
    /// When resuming from a previous WaitForUserInput (detected by presence of
    /// assistant messages in history), auto-search is skipped to avoid redundant
    /// queries and duplicate context injection.
    pub async fn initialize(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        event_tx: &Option<mpsc::Sender<StreamEvent>>,
    ) {
        let phase = self.state.current_phase();

        // Detect resumed execution: if history already contains assistant messages,
        // this is not a fresh start — skip auto-search to avoid redundant work.
        let is_resumed = messages.iter().any(|m| m.role == MessageRole::Assistant);

        if phase == AgentPhase::Research && !is_resumed {
            if let Some(search_ctx) = self.run_auto_search(messages).await {
                messages.push(ChatMessage::system(search_ctx));
            }
        }

        let entry = {
            let def = &self.definitions[&phase];
            (def.entry_prompt)(self.state.context())
        };
        messages.push(ChatMessage::system(entry));

        if let Some(ref tx) = event_tx {
            let _ = tx
                .send(StreamEvent::phase_transition("init", phase.to_string()))
                .await;
        }
    }

    /// Pre-step: check limits, inject prompts, return filtered `RuntimeContext`.
    /// Returns `None` when the loop should break (Done / limits).
    pub async fn pre_step(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        global_max: u32,
        event_tx: &Option<mpsc::Sender<StreamEvent>>,
    ) -> Option<RuntimeContext> {
        let phase = self.state.current_phase();

        if phase == AgentPhase::Done || self.state.is_at_global_limit(global_max) {
            return None;
        }

        let def = &self.definitions[&phase];

        if self.state.is_at_hard_limit(def) {
            let from = phase;
            match self.state.force_transition(def) {
                Ok(to) => {
                    warn!(
                        "[PhaseController] Hard limit in {}, forcing to {}",
                        from, to
                    );
                    messages.push(ChatMessage::system(PhasePrompt::hard_limit_prompt(
                        from, to,
                    )));
                    let to_def = &self.definitions[&to];
                    messages.push(ChatMessage::system((to_def.entry_prompt)(
                        self.state.context(),
                    )));
                    if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(StreamEvent::phase_transition(
                                from.to_string(),
                                to.to_string(),
                            ))
                            .await;
                    }
                }
                Err(_) => return None,
            }
        }

        if self.state.is_at_soft_limit(def) {
            messages.push(ChatMessage::system(PhasePrompt::soft_limit_prompt(
                self.state.current_phase(),
            )));
        }

        Some(self.build_filtered_context())
    }

    /// Post-step: classify, handle transitions.
    pub async fn post_step(
        &mut self,
        result: &StepResult,
        messages: &mut Vec<ChatMessage>,
        _msg_count_before: usize,
        event_tx: &Option<mpsc::Sender<StreamEvent>>,
    ) -> PhaseAction {
        let action = StepAction::classify(result);

        // WaitForUserInput should not consume an iteration — the LLM is pausing
        // for user interaction, not doing work.
        if !matches!(action, StepAction::WaitForUserInput) {
            self.state.advance_iteration();
        }

        // Build execution trace from tool results
        for tr in &result.tool_results {
            record_tool_in_trace(&mut self.phase_trace, tr);
        }

        match action {
            StepAction::PhaseTransition {
                to,
                context_summary,
            } => {
                let from = self.state.current_phase();
                let def = &self.definitions[&from];
                info!("[PhaseController] Phase transition requested: {} -> {}", from, to);

                // Layer 1: allowed_transitions check
                if !def.allowed_transitions.contains(&to) {
                    let reason = format!(
                        "不允许从 {} 转换到 {}。允许的目标：{}",
                        from, to,
                        def.allowed_transitions.iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    warn!("[PhaseController] Transition rejected: {}", reason);
                    return PhaseAction::Reject(reason);
                }

                // Layer 2: hard gates — engine enforced, non-negotiable
                let gate_failures: Vec<String> = def.hard_gates.iter()
                    .filter_map(|g| match g.check(&self.phase_trace) {
                        super::agent_phase::GateResult::Passed => None,
                        super::agent_phase::GateResult::Failed(reason) => Some(reason),
                    })
                    .collect();

                if !gate_failures.is_empty() {
                    let reason = format!(
                        "阶段门禁未通过：\n{}",
                        gate_failures.join("\n")
                    );
                    warn!("[PhaseController] Gate blocked transition: {}", reason);
                    return PhaseAction::Reject(reason);
                }

                // Layer 3: soft checklist — auto-verify what we can
                let unchecked: Vec<&str> = def.exit_checklist.iter()
                    .filter(|item| {
                        match &item.auto_verify {
                            Some(check) => !check(&self.phase_trace),
                            None => false, // can't auto-verify → don't block
                        }
                    })
                    .map(|i| i.label.as_str())
                    .collect();

                if !unchecked.is_empty() {
                    // Log warning but don't block — soft check
                    warn!(
                        "[PhaseController] Soft checklist items unchecked: {}",
                        unchecked.join(", ")
                    );
                }

                // All checks passed — execute transition
                if let Some(summary) = context_summary {
                    if !summary.is_empty() {
                        self.state.add_context(summary);
                    }
                }

                self.state.transition_def(def, to)
                    .expect("layer 1 already validated allowed_transitions");
                self.phase_trace = PhaseContext::new();

                // Inject entry prompt for the new phase
                let to_def = &self.definitions[&to];
                messages.push(ChatMessage::system((to_def.entry_prompt)(
                    self.state.context(),
                )));

                if let Some(ref tx) = event_tx {
                    let _ = tx
                        .send(StreamEvent::phase_transition(
                            from.to_string(),
                            to.to_string(),
                        ))
                        .await;
                }

                PhaseAction::Transition
            }
            StepAction::WaitForUserInput => {
                let current = self.state.current_phase();
                debug!("[PhaseController] WaitForUserInput in phase {}", current);

                // Save context summary so user reply can resume with context
                if current != AgentPhase::Execute && current != AgentPhase::Done {
                    self.state.add_context(format!(
                        "与用户交互暂停，等待用户回复以继续 {} 阶段。",
                        current
                    ));
                }

                if current == AgentPhase::Execute {
                    let exec_def = &self.definitions[&AgentPhase::Execute];
                    self.state.transition_def(exec_def, AgentPhase::Done)
                        .expect("Execute -> Done is allowed by definition");
                    if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(StreamEvent::phase_transition(
                                "execute".to_string(),
                                "done".to_string(),
                            ))
                            .await;
                    }
                }

                PhaseAction::Interrupt(if current != AgentPhase::Execute {
                    Some(current.to_string())
                } else {
                    None
                })
            }
            StepAction::Continue => PhaseAction::Continue,
        }
    }

    pub fn current_phase(&self) -> AgentPhase {
        self.state.current_phase()
    }

    // ── Internal helpers ────────────────────────────────────────────

    async fn run_auto_search(&self, messages: &[ChatMessage]) -> Option<String> {
        let query = ResearchContext::build_search_query(messages);
        if query.trim().is_empty() {
            return None;
        }

        debug!("[PhaseController] Auto-search query: '{}'", query);

        let tool_ctx = ToolContext::default();
        let wiki_result = match self
            .ctx
            .tools
            .execute(
                "wiki_search",
                serde_json::json!({"query": query, "limit": 5}),
                &tool_ctx,
            )
            .await
        {
            Ok(output) if !output.content.starts_with("No wiki pages found") => {
                Some(output.content)
            }
            _ => None,
        };

        wiki_result.map(|wiki| {
            format!(
                "[Research Context — 自动检索]\n\n{}\n\n\
                 你可以用 wiki_read 查看完整页面，或 wiki_search 调整搜索方向。\n\
                 需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。",
                wiki
            )
        })
    }

    fn build_filtered_context(&self) -> RuntimeContext {
        let phase = self.state.current_phase();
        let def = &self.definitions[&phase];
        let allowed = (def.allowed_tools)();
        if allowed.is_empty() {
            return self.ctx.clone();
        }
        let filtered = self.ctx.tools.filtered(&allowed);
        RuntimeContext {
            tools: Arc::new(filtered),
            ..self.ctx.clone()
        }
    }
}

// ── Trace recording (extracted for unit testing) ───────────────────

/// Record a tool invocation into the phase trace.
///
/// For `write_file` / `wiki_write` tools, the real path is parsed out of
/// `tr.arguments` so gates and checklists can reason about actual outputs
/// rather than just tool-name occurrences. Unknown tools only increment
/// `tools_invoked`.
fn record_tool_in_trace(trace: &mut super::agent_phase::PhaseContext, tr: &ToolCallResult) {
    trace.tools_invoked.push(tr.tool_name.clone());
    match tr.tool_name.as_str() {
        "write_file" => {
            if let Some(path) = tr.arguments.get("file_path").and_then(|v| v.as_str()) {
                trace.files_written.push(path.to_string());
            }
        }
        "wiki_write" => {
            if let Some(path) = tr.arguments.get("path").and_then(|v| v.as_str()) {
                trace.wiki_pages_written.push(path.to_string());
            }
        }
        _ => {}
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starting_at_research() {
        let sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        assert_eq!(sm.current_phase(), AgentPhase::Research);
        assert_eq!(sm.iteration_in_phase, 0);
    }

    #[test]
    fn test_starting_at_execute() {
        let sm = PhaseStateMachine::starting_at(AgentPhase::Execute);
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
    }

    #[test]
    fn test_valid_transition() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.transition_def(&defs[&AgentPhase::Research], AgentPhase::Execute).unwrap();
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
        assert_eq!(sm.iteration_in_phase, 0);
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        assert!(sm.transition_def(&defs[&AgentPhase::Research], AgentPhase::Review).is_err());
    }

    #[test]
    fn test_iteration_tracking() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.advance_iteration();
        sm.advance_iteration();
        assert_eq!(sm.iteration_in_phase, 2);
        sm.transition_def(&defs[&AgentPhase::Research], AgentPhase::Execute).unwrap();
        assert_eq!(sm.iteration_in_phase, 0);
        assert_eq!(sm.total_iterations, 2);
    }

    #[test]
    fn test_soft_limit() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..5 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_soft_limit(&defs[&AgentPhase::Research]));
        assert!(!sm.is_at_hard_limit(&defs[&AgentPhase::Research]));
    }

    #[test]
    fn test_hard_limit() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_hard_limit(&defs[&AgentPhase::Research]));
    }

    #[test]
    fn test_force_transition() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert_eq!(sm.force_transition(&defs[&AgentPhase::Research]).unwrap(), AgentPhase::Execute);
    }

    #[test]
    fn test_context_accumulation() {
        let defs = default_definitions();
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.add_context("Found wiki pages".into());
        sm.transition_def(&defs[&AgentPhase::Research], AgentPhase::Execute).unwrap();
        sm.add_context("Executed plan".into());
        assert!(sm.context().format().contains("Found wiki pages"));
    }

    #[test]
    fn test_global_limit() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..99 {
            sm.advance_iteration();
        }
        assert!(!sm.is_at_global_limit(100));
        sm.advance_iteration();
        assert!(sm.is_at_global_limit(100));
    }

    // ── record_tool_in_trace ─────────────────────────────────────────

    use crate::kernel::phased::agent_phase::PhaseContext;
    use crate::tools::ToolControlSignal;

    fn make_tool_result(name: &str, args: serde_json::Value) -> ToolCallResult {
        ToolCallResult {
            tool_call_id: "call_x".into(),
            tool_name: name.into(),
            arguments: args,
            output: String::new(),
            signal: None::<ToolControlSignal>,
        }
    }

    #[test]
    fn test_record_write_file_stores_real_path() {
        let mut trace = PhaseContext::new();
        let tr = make_tool_result(
            "write_file",
            serde_json::json!({"file_path": "topics/plans/foo.md", "content": "x"}),
        );
        record_tool_in_trace(&mut trace, &tr);
        assert_eq!(trace.tools_invoked, vec!["write_file"]);
        assert_eq!(trace.files_written, vec!["topics/plans/foo.md"]);
        assert!(trace.wiki_pages_written.is_empty());
    }

    #[test]
    fn test_record_wiki_write_stores_real_path() {
        let mut trace = PhaseContext::new();
        let tr = make_tool_result(
            "wiki_write",
            serde_json::json!({"path": "topics/rust-async", "title": "T", "body": "b"}),
        );
        record_tool_in_trace(&mut trace, &tr);
        assert_eq!(trace.wiki_pages_written, vec!["topics/rust-async"]);
        assert!(trace.files_written.is_empty());
    }

    #[test]
    fn test_record_unknown_tool_only_tracks_invocation() {
        let mut trace = PhaseContext::new();
        let tr = make_tool_result("wiki_search", serde_json::json!({"query": "q"}));
        record_tool_in_trace(&mut trace, &tr);
        assert_eq!(trace.tools_invoked, vec!["wiki_search"]);
        assert!(trace.files_written.is_empty());
        assert!(trace.wiki_pages_written.is_empty());
    }

    #[test]
    fn test_record_write_file_with_missing_path_is_noop_for_files() {
        let mut trace = PhaseContext::new();
        // Malformed args from LLM — tool would have errored, but trace must not panic
        let tr = make_tool_result("write_file", serde_json::json!({"content": "x"}));
        record_tool_in_trace(&mut trace, &tr);
        assert_eq!(trace.tools_invoked, vec!["write_file"]);
        assert!(trace.files_written.is_empty());
    }

    // ── PhaseDefinition gate/checklist behavior ──────────────────────

    #[test]
    fn test_planning_def_files_written_checklist_auto_verifies() {
        let defs = default_definitions();
        let planning = &defs[&AgentPhase::Planning];
        // Planning has the auto-verifiable item "计划已保存为文件"
        let auto_item = planning
            .exit_checklist
            .iter()
            .find(|i| i.auto_verify.is_some())
            .expect("planning should have auto-verifiable item");

        let mut trace = PhaseContext::new();
        assert!(!(auto_item.auto_verify.unwrap())(&trace), "empty trace must fail check");

        trace.files_written.push("topics/plans/foo.md".into());
        assert!((auto_item.auto_verify.unwrap())(&trace), "non-empty trace must pass");
    }

    #[test]
    fn test_planning_def_hard_gate_present() {
        let defs = default_definitions();
        let planning = &defs[&AgentPhase::Planning];
        assert_eq!(planning.hard_gates.len(), 1, "planning must have FilesWrittenGate");
        // Empty trace → gate fails with reason
        let trace = PhaseContext::new();
        let result = planning.hard_gates[0].check(&trace);
        assert!(matches!(result, super::super::agent_phase::GateResult::Failed(_)));
    }

    #[test]
    fn test_other_phases_have_no_hard_gates() {
        let defs = default_definitions();
        for phase in [
            AgentPhase::Research,
            AgentPhase::Execute,
            AgentPhase::Review,
            AgentPhase::Done,
        ] {
            assert!(
                defs[&phase].hard_gates.is_empty(),
                "{phase} must not have hard gates per design spec"
            );
        }
    }
}
