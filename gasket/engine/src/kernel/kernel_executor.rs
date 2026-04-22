//! Kernel executor — core LLM loop.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::kernel::{
    context::{KernelConfig, RuntimeContext},
    error::KernelError,
    steppable_executor::SteppableExecutor,
    stream::StreamEvent,
};
use crate::token_tracker::TokenUsage;
use crate::tools::{SubagentSpawner, ToolRegistry};
use crate::vault::redact_secrets;
use gasket_providers::{ChatMessage, ChatResponse, LlmProvider};

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
    pub token_usage: Option<gasket_types::TokenUsage>,
    pub cost: Option<f64>,
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
            cost: None,
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

/// Kernel executor - core LLM loop
pub struct KernelExecutor {
    ctx: RuntimeContext,
}

impl KernelExecutor {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self {
            ctx: RuntimeContext::new(provider, tools, config),
        }
    }

    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.ctx.spawner = Some(spawner);
        self
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.ctx.token_tracker = Some(tracker);
        self
    }

    pub fn with_checkpoint(
        mut self,
        callback: Arc<dyn Fn(usize) -> Option<String> + Send + Sync>,
    ) -> Self {
        self.ctx.checkpoint_callback = Some(callback);
        self
    }

    pub async fn execute_with_options(
        &self,
        messages: Vec<ChatMessage>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        self.execute_internal(messages, None, options).await
    }

    pub async fn execute_stream_with_options(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        self.execute_internal(messages, Some(event_tx), options)
            .await
    }

    async fn execute_internal(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: Option<mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        let mut state = ExecutionState::new(messages);
        let mut ledger = TokenLedger::new();

        let result = self
            .run_loop(&mut state, &mut ledger, event_tx.as_ref(), options)
            .await;

        // Ensure Done is sent exactly once
        if let Some(ref tx) = event_tx {
            let _ = tx.send(StreamEvent::done()).await;
        }

        result
    }

    async fn run_loop(
        &self,
        state: &mut ExecutionState,
        ledger: &mut TokenLedger,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        let steppable = SteppableExecutor::new(self.ctx.clone());

        for iteration in 1..=self.ctx.config.max_iterations {
            debug!("[Kernel] iteration {}", iteration);

            let result = steppable
                .step(&mut state.messages, ledger, event_tx)
                .await?;

            // Track tools used from this step
            for tr in &result.tool_results {
                state.tools_used.push(tr.tool_name.clone());
            }

            Self::log_token_usage(ledger, iteration);
            Self::log_response(&result.response, iteration, options.vault_values);

            if !result.should_continue {
                let content = result
                    .response
                    .content
                    .clone()
                    .unwrap_or_else(|| DEFAULT_NO_RESPONSE.to_string());
                let reasoning = result.response.reasoning_content.clone();
                return Ok(state.to_result(content, reasoning, ledger));
            }
        }

        info!(
            "[Kernel] Max iterations ({}) reached",
            self.ctx.config.max_iterations
        );
        Err(KernelError::MaxIterations(self.ctx.config.max_iterations))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_ledger_accumulate_usage() {
        let mut ledger = TokenLedger::new();
        assert!(ledger.total_usage.is_none());

        let usage1 = gasket_types::TokenUsage::new(100, 50);
        ledger.accumulate(&usage1);
        assert_eq!(ledger.total_usage.as_ref().unwrap().input_tokens, 100);
        assert_eq!(ledger.total_usage.as_ref().unwrap().output_tokens, 50);

        let usage2 = gasket_types::TokenUsage::new(200, 100);
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
