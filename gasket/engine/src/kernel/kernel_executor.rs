//! Kernel executor — core LLM loop.
//!
//! Thin wrapper around `SteppableExecutor` providing high-level
//! `execute_with_options` / `execute_stream_with_options` entry points.
//! The multi-turn loop logic lives here; single-step logic is in `SteppableExecutor`.

use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::kernel::{
    context::RuntimeContext, error::KernelError, steppable_executor::SteppableExecutor,
    stream::StreamEvent,
};
use crate::token_tracker::TokenUsage;
use crate::vault::redact_secrets;
use gasket_providers::{ChatMessage, ChatResponse};

/// Default response when no content is available
const DEFAULT_NO_RESPONSE: &str = "I've completed processing but have no response to give.";

/// Options for executor behavior
#[derive(Default)]
pub struct ExecutorOptions<'a> {
    pub vault_values: &'a [String],
}

impl<'a> ExecutorOptions<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_vault_values(mut self, values: &'a [String]) -> Self {
        self.vault_values = values;
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
    /// contains the phase name (e.g. "research"). The session layer uses this
    /// to restore the phase on the next user message. `None` for normal completion.
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
}

/// Token usage ledger — separated from ExecutionState for purity.
///
/// Each iteration returns its `TokenUsage` delta; this ledger accumulates
/// them across the full execution lifecycle.
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
///
/// Holds a `RuntimeContext` and provides `execute_with_options` /
/// `execute_stream_with_options`. The actual loop logic is in
/// `run_loop` at module scope.
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

/// Core multi-turn LLM loop.
///
/// Iterates up to `max_iterations`, calling `SteppableExecutor::step()` each turn.
/// Sends `StreamEvent::done()` exactly once when finished.
async fn run_loop(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: Option<mpsc::Sender<StreamEvent>>,
    options: &ExecutorOptions<'_>,
) -> Result<ExecutionResult, KernelError> {
    let mut state = ExecutionState::new(messages);
    let mut ledger = TokenLedger::new();
    let steppable = SteppableExecutor::new(ctx.clone());

    for iteration in 1..=ctx.config.max_iterations {
        debug!("[Kernel] iteration {}", iteration);

        let result = steppable
            .step(&mut state.messages, &mut ledger, event_tx.as_ref())
            .await?;

        for tr in &result.tool_results {
            state.tools_used.push(tr.tool_name.clone());
        }

        log_token_usage(&ledger, iteration);
        log_response(&result.response, iteration, options.vault_values);

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

fn log_response(response: &ChatResponse, iteration: u32, vault_values: &[String]) {
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
        let opts = ExecutorOptions::new().with_vault_values(&vault);
        assert_eq!(opts.vault_values.len(), 1);
    }
}
