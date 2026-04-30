//! Phase controller — strategy object injected into the unified `run_loop`.
//!
//! Encapsulates all phased-execution logic so `kernel_executor::run_loop`
//! remains a single loop with optional phase hooks.

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::kernel::context::RuntimeContext;
use crate::kernel::steppable_executor::StepResult;
use crate::kernel::stream::StreamEvent;
use crate::tools::ToolContext;

use gasket_providers::ChatMessage;
use tokio::sync::mpsc;

use super::agent_phase::AgentPhase;
use super::phase_prompt::{ContextAccumulator, PhasePrompt};
use super::research_context::ResearchContext;
use super::step_action::StepAction;

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
}

/// Encapsulates all phased-execution logic as a strategy for `run_loop`.
pub struct PhaseController {
    state: PhaseStateMachine,
    ctx: RuntimeContext,
}

impl PhaseController {
    pub fn new(ctx: &RuntimeContext, start_phase: Option<AgentPhase>) -> Self {
        let initial = start_phase.unwrap_or(AgentPhase::Research);
        Self {
            state: PhaseStateMachine::starting_at(initial),
            ctx: ctx.clone(),
        }
    }

    /// One-time init: auto-search + entry prompt + initial event.
    pub async fn initialize(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        event_tx: &Option<mpsc::Sender<StreamEvent>>,
    ) {
        let phase = self.state.current_phase();

        if phase == AgentPhase::Research {
            if let Some(search_ctx) = self.run_auto_search(messages).await {
                messages.push(ChatMessage::system(search_ctx));
            }
        }

        let entry = PhasePrompt::entry_prompt(phase, self.state.context());
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

        if self.state.is_at_hard_limit() {
            let from = phase;
            match self.state.force_transition() {
                Ok(to) => {
                    warn!(
                        "[PhaseController] Hard limit in {}, forcing to {}",
                        from, to
                    );
                    messages.push(ChatMessage::system(PhasePrompt::hard_limit_prompt(from, to)));
                    messages.push(ChatMessage::system(PhasePrompt::entry_prompt(
                        to,
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

        if self.state.is_at_soft_limit() {
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
        self.state.advance_iteration();
        let action = StepAction::classify(result);

        match action {
            StepAction::PhaseTransition { to, context_summary } => {
                let from = self.state.current_phase();
                info!("[PhaseController] Phase transition: {} -> {}", from, to);

                if let Some(summary) = context_summary {
                    if !summary.is_empty() {
                        self.state.add_context(summary);
                    }
                }

                self.state.transition(to).ok();

                // Inject entry prompt for the new phase — full history is preserved.
                // ContextCompactor handles compression when needed.
                messages.push(ChatMessage::system(PhasePrompt::entry_prompt(
                    to,
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
                debug!(
                    "[PhaseController] WaitForUserInput in phase {}",
                    current
                );

                if current == AgentPhase::Execute {
                    self.state.transition(AgentPhase::Done).ok();
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
            Ok(output) if !output.content.starts_with("No wiki pages found") => Some(output.content),
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
        let allowed = self.state.current_phase().allowed_tools();
        if allowed.is_empty() {
            return self.ctx.clone();
        }
        let filtered = self.ctx.tools.filtered(allowed);
        RuntimeContext {
            tools: Arc::new(filtered),
            ..self.ctx.clone()
        }
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
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
        assert_eq!(sm.iteration_in_phase, 0);
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        assert!(sm.transition(AgentPhase::Review).is_err());
    }

    #[test]
    fn test_iteration_tracking() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.advance_iteration();
        sm.advance_iteration();
        assert_eq!(sm.iteration_in_phase, 2);
        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.iteration_in_phase, 0);
        assert_eq!(sm.total_iterations, 2);
    }

    #[test]
    fn test_soft_limit() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..5 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_soft_limit());
        assert!(!sm.is_at_hard_limit());
    }

    #[test]
    fn test_hard_limit() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_hard_limit());
    }

    #[test]
    fn test_force_transition() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert_eq!(sm.force_transition().unwrap(), AgentPhase::Execute);
    }

    #[test]
    fn test_context_accumulation() {
        let mut sm = PhaseStateMachine::starting_at(AgentPhase::Research);
        sm.add_context("Found wiki pages".into());
        sm.transition(AgentPhase::Execute).unwrap();
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
}
