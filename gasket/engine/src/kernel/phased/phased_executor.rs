use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::kernel::context::RuntimeContext;
use crate::kernel::kernel_executor::{ExecutionResult, TokenLedger};
use crate::kernel::steppable_executor::SteppableExecutor;
use crate::kernel::KernelError;
use crate::tools::ToolContext;

use gasket_providers::{ChatMessage, MessageRole};
use gasket_types::StreamEvent;

use super::agent_phase::AgentPhase;
use super::phase_prompt::{ContextAccumulator, PhasePrompt};
use super::research_context::ResearchContext;
use super::step_action::StepAction;

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

    /// Create a state machine starting at the given phase (for re-entrant sessions).
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
    pub fn iteration_in_phase(&self) -> u32 {
        self.iteration_in_phase
    }
    pub fn total_iterations(&self) -> u32 {
        self.total_iterations
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

/// Main entry point for phased execution.
pub struct PhasedExecutor {
    ctx: RuntimeContext,
}

impl PhasedExecutor {
    pub fn new(ctx: RuntimeContext) -> Self {
        Self { ctx }
    }

    /// Execute the phased agent loop.
    ///
    /// `start_phase` allows resuming a previously interrupted phased session
    /// (e.g. when the LLM asked for clarification during Research).
    /// When `None`, starts from Research with auto-search.
    pub async fn run(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
        start_phase: Option<AgentPhase>,
    ) -> Result<ExecutionResult, KernelError> {
        let initial_phase = start_phase.unwrap_or(AgentPhase::Research);
        let mut state = PhaseStateMachine::starting_at(initial_phase);
        let mut ledger = TokenLedger::new();
        let mut all_messages = messages;
        let mut tools_used = Vec::new();

        // Auto-search only for fresh Research starts
        if initial_phase == AgentPhase::Research {
            if let Some(search_context) = self.run_auto_search(&all_messages).await {
                all_messages.push(ChatMessage::system(search_context));
            }
        }

        // Inject entry prompt for the starting phase
        let entry = PhasePrompt::entry_prompt(initial_phase, state.context());
        all_messages.push(ChatMessage::system(entry));

        // Send initial phase event
        let _ = event_tx
            .send(StreamEvent::phase_transition("init", initial_phase.to_string()))
            .await;

        loop {
            let current_phase = state.current_phase();

            // --- Terminal state ---
            if current_phase == AgentPhase::Done {
                let _ = event_tx.send(StreamEvent::done()).await;
                return Ok(ExecutionResult {
                    content: all_messages
                        .iter()
                        .rev()
                        .find_map(|m| {
                            if m.role == MessageRole::Assistant {
                                m.content.clone()
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default(),
                    reasoning_content: None,
                    tools_used,
                    token_usage: ledger.total_usage.clone(),
                    interrupted_phase: None,
                });
            }

            // --- Global iteration limit ---
            if state.is_at_global_limit(self.ctx.config.max_iterations) {
                warn!(
                    "[PhasedExecutor] Global max iterations ({}) reached in phase {}",
                    self.ctx.config.max_iterations, current_phase
                );
                let _ = event_tx.send(StreamEvent::done()).await;
                return Ok(ExecutionResult {
                    content: "达到最大迭代次数，任务执行被截断。".to_string(),
                    reasoning_content: None,
                    tools_used,
                    token_usage: ledger.total_usage.clone(),
                    interrupted_phase: None,
                });
            }

            // --- Hard limit for current phase ---
            if state.is_at_hard_limit() {
                let from = current_phase;
                match state.force_transition() {
                    Ok(to) => {
                        warn!(
                            "[PhasedExecutor] Hard limit in {}, forcing to {}",
                            from, to
                        );
                        let prompt = PhasePrompt::hard_limit_prompt(from, to);
                        all_messages.push(ChatMessage::system(prompt));
                        let entry = PhasePrompt::entry_prompt(to, state.context());
                        all_messages.push(ChatMessage::system(entry));
                        let _ = event_tx
                            .send(StreamEvent::phase_transition(
                                from.to_string(),
                                to.to_string(),
                            ))
                            .await;
                        continue;
                    }
                    Err(_) => {
                        let _ = event_tx.send(StreamEvent::done()).await;
                        return Ok(ExecutionResult {
                            content: "达到迭代上限，任务执行被截断。".to_string(),
                            reasoning_content: None,
                            tools_used,
                            token_usage: ledger.total_usage.clone(),
                            interrupted_phase: None,
                        });
                    }
                }
            }

            // --- Soft limit injection ---
            if state.is_at_soft_limit() {
                let prompt = PhasePrompt::soft_limit_prompt(current_phase);
                all_messages.push(ChatMessage::system(prompt));
            }

            // --- Build filtered context for this step ---
            let phased_ctx = self.build_filtered_context(current_phase);

            // --- Execute one step ---
            let steppable = SteppableExecutor::new(phased_ctx);
            let result = steppable
                .step(&mut all_messages, &mut ledger, Some(&event_tx))
                .await
                .map_err(|e| KernelError::Provider(e.to_string()))?;

            for tr in &result.tool_results {
                tools_used.push(tr.tool_name.clone());
            }

            state.advance_iteration();

            // --- Classify the step result ---
            let action = StepAction::classify(&result);

            match action {
                StepAction::PhaseTransition { to } => {
                    let from = current_phase;
                    info!("[PhasedExecutor] Phase transition: {} -> {}", from, to);

                    // Extract context_summary from phase_transition tool call arguments
                    if let Some(tc) = result
                        .response
                        .tool_calls
                        .iter()
                        .find(|tc| tc.function.name == "phase_transition")
                    {
                        if let Some(summary) = tc
                            .function
                            .arguments
                            .get("context_summary")
                            .and_then(|v| v.as_str())
                        {
                            if !summary.is_empty() {
                                state.add_context(summary.to_string());
                            }
                        }
                    }

                    state
                        .transition(to)
                        .map_err(|e| KernelError::Provider(e))?;

                    // Inject next phase entry prompt
                    let entry = PhasePrompt::entry_prompt(to, state.context());
                    all_messages.push(ChatMessage::system(entry));

                    // Emit phase transition event
                    let _ = event_tx
                        .send(StreamEvent::phase_transition(
                            from.to_string(),
                            to.to_string(),
                        ))
                        .await;

                    continue;
                }
                StepAction::WaitForUserInput => {
                    debug!(
                        "[PhasedExecutor] LLM sent text without tool calls in phase {} — treating as phase completion",
                        current_phase
                    );

                    // If in Execute phase with text response, auto-transition to Done
                    if current_phase == AgentPhase::Execute {
                        state
                            .transition(AgentPhase::Done)
                            .map_err(|e| KernelError::Provider(e))?;
                        let _ = event_tx
                            .send(StreamEvent::phase_transition(
                                "execute".to_string(),
                                "done".to_string(),
                            ))
                            .await;
                    }

                    // For non-Execute phases, mark the interrupted phase so the
                    // session layer can resume on the next user message.
                    let interrupted = if current_phase != AgentPhase::Execute {
                        Some(current_phase.to_string())
                    } else {
                        None
                    };

                    let _ = event_tx.send(StreamEvent::done()).await;
                    return Ok(ExecutionResult {
                        content: result.response.content.clone().unwrap_or_default(),
                        reasoning_content: result.response.reasoning_content.clone(),
                        tools_used,
                        token_usage: ledger.total_usage.clone(),
                        interrupted_phase: interrupted,
                    });
                }
                StepAction::Continue => {
                    // Normal tool execution — loop continues
                }
            }
        }
    }

    /// Run auto-search at the start of Research phase.
    ///
    /// Calls `wiki_search` via the ToolRegistry and formats the result
    /// as a structured system message for injection.
    async fn run_auto_search(&self, messages: &[ChatMessage]) -> Option<String> {
        let query = ResearchContext::build_search_query(messages);
        if query.trim().is_empty() {
            debug!("[PhasedExecutor] Auto-search skipped: empty query");
            return None;
        }

        debug!("[PhasedExecutor] Auto-search query: '{}'", query);

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
            Ok(output) if !output.starts_with("No wiki pages found") => Some(output),
            Ok(_) => {
                debug!("[PhasedExecutor] Auto-search: no wiki results");
                None
            }
            Err(e) => {
                debug!("[PhasedExecutor] Auto-search wiki_search failed: {}", e);
                None
            }
        };

        if wiki_result.is_none() {
            return None;
        }

        Some(format!(
            "[Research Context — 自动检索]\n\n{}\n\n\
             你可以用 wiki_read 查看完整页面，或 wiki_search 调整搜索方向。\n\
             需要更多信息也可以直接问我。信息充分后调用 phase_transition 进入下一阶段。",
            wiki_result.unwrap()
        ))
    }

    /// Build a filtered RuntimeContext with only allowed tools for the current phase.
    fn build_filtered_context(&self, phase: AgentPhase) -> RuntimeContext {
        let allowed = phase.allowed_tools();

        if allowed.is_empty() {
            return self.ctx.clone();
        }

        let filtered_registry = self.ctx.tools.filtered(allowed);
        RuntimeContext {
            tools: Arc::new(filtered_registry),
            ..self.ctx.clone()
        }
    }
}

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
    fn test_starting_at() {
        let sm = PhaseStateMachine::starting_at(AgentPhase::Execute);
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
        assert_eq!(sm.iteration_in_phase(), 0);
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
        for _ in 0..5 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_soft_limit());
        assert!(!sm.is_at_hard_limit());
    }

    #[test]
    fn test_hard_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_hard_limit());
    }

    #[test]
    fn test_force_transition() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 {
            sm.advance_iteration();
        }
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
        for _ in 0..99 {
            sm.advance_iteration();
        }
        assert!(!sm.is_at_global_limit(100));
        sm.advance_iteration();
        assert!(sm.is_at_global_limit(100));
    }
}
