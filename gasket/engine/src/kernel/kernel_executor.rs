//! Kernel executor — unified LLM loop with optional phase strategy.
//!
//! Single `run_loop` handles both phased and non-phased execution.
//! Phase-specific logic is encapsulated in `PhaseController`.

use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::kernel::{
    context::RuntimeContext, error::KernelError, phased::phase_controller::PhaseController,
    steppable_executor::SteppableExecutor, stream::StreamEvent,
};
use crate::token_tracker::TokenUsage;
use crate::vault::redact_secrets;
use gasket_providers::ChatMessage;

/// Default response when no content is available
const DEFAULT_NO_RESPONSE: &str = "I've completed processing but have no response to give.";

/// Options for executor behavior
#[derive(Default)]
pub struct ExecutorOptions<'a> {
    pub vault_values: &'a [String],
    /// Resume phased execution from this phase (None = start from Research or non-phased).
    pub start_phase: Option<&'a str>,
}

impl<'a> ExecutorOptions<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_vault_values(mut self, values: &'a [String]) -> Self {
        self.vault_values = values;
        self
    }

    pub fn with_start_phase(mut self, phase: Option<&'a str>) -> Self {
        self.start_phase = phase;
        self
    }
}

/// Agent execution result
#[derive(Debug)]
pub struct ExecutionResult {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub tools_used: Vec<String>,
    pub token_usage: Option<TokenUsage>,
    /// When phased execution is interrupted by `WaitForUserInput`, this field
    /// contains the phase name (e.g. "research"). `None` for normal completion.
    pub interrupted_phase: Option<String>,
}

/// Accumulated execution state — message history and tool tracking only.
/// Token accounting is handled separately by `TokenLedger`.
struct ExecutionState {
    messages: Vec<ChatMessage>,
    tools_used: Vec<String>,
}

impl ExecutionState {
    fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            tools_used: Vec::new(),
        }
    }

    fn to_result(
        &self,
        content: String,
        reasoning_content: Option<String>,
        ledger: &TokenLedger,
    ) -> ExecutionResult {
        ExecutionResult {
            content,
            reasoning_content,
            tools_used: self.tools_used.clone(),
            token_usage: ledger.total_usage.clone(),
            interrupted_phase: None,
        }
    }

    fn to_result_with_interrupt(
        &self,
        content: String,
        reasoning_content: Option<String>,
        ledger: &TokenLedger,
        interrupted_phase: Option<String>,
    ) -> ExecutionResult {
        ExecutionResult {
            content,
            reasoning_content,
            tools_used: self.tools_used.clone(),
            token_usage: ledger.total_usage.clone(),
            interrupted_phase,
        }
    }
}

/// Token usage ledger — separated from ExecutionState for purity.
pub struct TokenLedger {
    pub total_usage: Option<TokenUsage>,
}

impl Default for TokenLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenLedger {
    pub fn new() -> Self {
        Self { total_usage: None }
    }

    pub fn accumulate(&mut self, usage: &TokenUsage) {
        self.total_usage = Some(match self.total_usage.take() {
            Some(mut acc) => {
                acc.input_tokens += usage.input_tokens;
                acc.output_tokens += usage.output_tokens;
                acc.total_tokens += usage.total_tokens;
                acc
            }
            None => usage.clone(),
        });
    }
}

/// Kernel executor — thin convenience wrapper.
pub struct KernelExecutor {
    ctx: RuntimeContext,
}

impl KernelExecutor {
    pub fn new(ctx: RuntimeContext) -> Self {
        Self { ctx }
    }

    pub async fn execute_with_options(
        &self,
        messages: Vec<ChatMessage>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        run_loop(&self.ctx, messages, None, options).await
    }

    pub async fn execute_stream_with_options(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        run_loop(&self.ctx, messages, Some(event_tx), options).await
    }
}

/// Unified multi-turn LLM loop.
///
/// When `ctx.config.phased_execution` is true, a `PhaseController` is created
/// to manage per-step phase logic (tool filtering, prompt injection, transitions).
/// Otherwise, the loop runs the standard SteppableExecutor cycle.
async fn run_loop(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: Option<mpsc::Sender<StreamEvent>>,
    options: &ExecutorOptions<'_>,
) -> Result<ExecutionResult, KernelError> {
    let mut state = ExecutionState::new(messages);
    let mut ledger = TokenLedger::new();

    // Create phase controller if phased execution is enabled
    let mut phase = if ctx.config.phased_execution {
        let start_phase = options
            .start_phase
            .and_then(|s| crate::kernel::phased::AgentPhase::try_from(s).ok());
        let mut ctrl = PhaseController::new(ctx, start_phase);
        ctrl.initialize(&mut state.messages, &event_tx).await;
        Some(ctrl)
    } else {
        None
    };

    for iteration in 1..=ctx.config.max_iterations {
        debug!("[Kernel] iteration {}", iteration);

        // --- Phase pre-step: limits, prompts, tool filtering ---
        let step_ctx = if let Some(ref mut p) = phase {
            match p
                .pre_step(&mut state.messages, ctx.config.max_iterations, &event_tx)
                .await
            {
                Some(ctx) => ctx,
                None => {
                    // Done or global limit
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::done()).await;
                    }
                    return Ok(state.to_result(
                        "达到迭代上限，任务执行被截断。".to_string(),
                        None,
                        &ledger,
                    ));
                }
            }
        } else {
            ctx.clone()
        };

        // --- Execute one step ---
        let steppable = SteppableExecutor::new(step_ctx);
        let msg_count_before = state.messages.len();
        let result = steppable
            .step(&mut state.messages, &mut ledger, event_tx.as_ref())
            .await?;

        for tr in &result.tool_results {
            state.tools_used.push(tr.tool_name.clone());
        }

        log_token_usage(&ledger, iteration);
        log_response(&result.response, iteration, options.vault_values);

        // --- Phase post-step: classify, transitions ---
        if let Some(ref mut p) = phase {
            use crate::kernel::phased::phase_controller::PhaseAction;
            match p
                .post_step(&result, &mut state.messages, msg_count_before, &event_tx)
                .await
            {
                PhaseAction::Continue => {}
                PhaseAction::Transition => continue,
                PhaseAction::Interrupt(interrupted) => {
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::done()).await;
                    }
                    return Ok(state.to_result_with_interrupt(
                        result.response.content.clone().unwrap_or_default(),
                        result.response.reasoning_content.clone(),
                        &ledger,
                        interrupted,
                    ));
                }
            }
        } else {
            // Non-phased: simple should_continue check
            if !result.should_continue {
                let content = result
                    .response
                    .content
                    .clone()
                    .unwrap_or_else(|| DEFAULT_NO_RESPONSE.to_string());
                let reasoning = result.response.reasoning_content.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(StreamEvent::done()).await;
                }
                return Ok(state.to_result(content, reasoning, &ledger));
            }
        }
    }

    // Max iterations reached
    info!(
        "[Kernel] Max iterations ({}) reached",
        ctx.config.max_iterations
    );
    if let Some(ref tx) = event_tx {
        let _ = tx.send(StreamEvent::done()).await;
    }
    Err(KernelError::MaxIterations(ctx.config.max_iterations))
}

fn log_token_usage(ledger: &TokenLedger, iteration: u32) {
    if let Some(ref usage) = ledger.total_usage {
        info!(
            "[Token] iter={} input={} output={} total={}",
            iteration, usage.input_tokens, usage.output_tokens, usage.total_tokens
        );
    }
}

fn log_response(
    response: &gasket_providers::ChatResponse,
    iteration: u32,
    vault_values: &[String],
) {
    if let Some(ref reasoning) = response.reasoning_content {
        if !reasoning.is_empty() {
            let safe = redact_secrets(reasoning, vault_values);
            debug!("[Kernel] Reasoning (iter {}): {}", iteration, safe);
        }
    }

    if let Some(ref content) = response.content {
        if !content.is_empty() {
            let safe = redact_secrets(content, vault_values);
            info!("[Kernel] Response (iter {}): {}", iteration, safe);
        }
    }

    info!(
        "[Kernel] iter {} has_tool_calls={}, tool_count={}",
        iteration,
        response.has_tool_calls(),
        response.tool_calls.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_ledger_accumulate_usage() {
        let mut ledger = TokenLedger::new();
        assert!(ledger.total_usage.is_none());

        let usage1 = TokenUsage::new(100, 50);
        ledger.accumulate(&usage1);
        assert_eq!(ledger.total_usage.as_ref().unwrap().input_tokens, 100);
        assert_eq!(ledger.total_usage.as_ref().unwrap().output_tokens, 50);

        let usage2 = TokenUsage::new(200, 100);
        ledger.accumulate(&usage2);
        assert_eq!(ledger.total_usage.as_ref().unwrap().input_tokens, 300);
        assert_eq!(ledger.total_usage.as_ref().unwrap().output_tokens, 150);
    }

    #[test]
    fn test_executor_options_builder() {
        let vault = vec!["secret".to_string()];
        let opts = ExecutorOptions::new()
            .with_vault_values(&vault)
            .with_start_phase(Some("research"));
        assert_eq!(opts.vault_values.len(), 1);
        assert_eq!(opts.start_phase, Some("research"));
    }
}
