//! Agent executor - the core LLM loop with optional enhancements
//!
//! This is the core execution engine that handles:
//! - LLM request/response cycle
//! - Tool call detection and execution
//! - Iteration control
//! - Token usage and cost calculation (optional)
//! - Log redaction (optional)
//! - Optional streaming via event channel
//!
//! It does NOT handle:
//! - Session persistence
//! - History management
//! - External hooks
//! - Vault injection

use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::agent::executor::ToolExecutor;
use crate::agent::loop_::AgentConfig;
use crate::agent::request::RequestHandler;
use crate::agent::stream::{self, StreamEvent};
use crate::error::AgentError;
use crate::providers::{ChatMessage, ChatResponse, ChatStream, LlmProvider};
use crate::token_tracker::{ModelPricing, TokenUsage};
use crate::tools::ToolRegistry;
use crate::vault::redact_secrets;

/// Default response when no content is available
const DEFAULT_NO_RESPONSE: &str = "I've completed processing but have no response to give.";
/// Default response when max iterations reached
const DEFAULT_MAX_ITERATIONS: &str = "Maximum iterations reached.";

// ── Configuration Types ─────────────────────────────────────

/// Options for executor behavior
#[derive(Default)]
pub struct ExecutorOptions<'a> {
    /// Pricing configuration for cost calculation
    pub pricing: Option<ModelPricing>,
    /// Vault values for log redaction
    pub vault_values: &'a [String],
}

impl<'a> ExecutorOptions<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pricing(mut self, pricing: ModelPricing) -> Self {
        self.pricing = Some(pricing);
        self
    }

    pub fn with_vault_values(mut self, values: &'a [String]) -> Self {
        self.vault_values = values;
        self
    }
}

// ── Execution Result ─────────────────────────────────────────

/// Agent execution result
#[derive(Debug)]
pub struct ExecutionResult {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub tools_used: Vec<String>,
    pub token_usage: Option<TokenUsage>,
    pub cost: f64,
}

// ── Internal State ──────────────────────────────────────────

/// Accumulated execution state
struct ExecutionState {
    messages: Vec<ChatMessage>,
    tools_used: Vec<String>,
    total_usage: Option<TokenUsage>,
    total_cost: f64,
}

impl ExecutionState {
    fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            tools_used: Vec::new(),
            total_usage: None,
            total_cost: 0.0,
        }
    }

    fn accumulate_usage(&mut self, usage: &TokenUsage, pricing: Option<&ModelPricing>) {
        let cost = pricing
            .map(|p| p.calculate_cost(usage.input_tokens, usage.output_tokens))
            .unwrap_or(0.0);

        self.total_usage = Some(match self.total_usage.take() {
            Some(mut acc) => {
                acc.input_tokens += usage.input_tokens;
                acc.output_tokens += usage.output_tokens;
                acc.total_tokens += usage.total_tokens;
                acc
            }
            None => usage.clone(),
        });
        self.total_cost += cost;
    }

    fn into_result(self, content: String, reasoning_content: Option<String>) -> ExecutionResult {
        ExecutionResult {
            content,
            reasoning_content,
            tools_used: self.tools_used,
            token_usage: self.total_usage,
            cost: self.total_cost,
        }
    }
}

// ── Iteration Result ────────────────────────────────────────

/// Result of a single LLM iteration
enum IterationOutcome {
    /// LLM returned final response (no tool calls)
    FinalResponse {
        content: String,
        reasoning_content: Option<String>,
    },
    /// LLM made tool calls, continue iteration
    ContinueWithTools,
    /// Max iterations reached
    MaxIterationsReached,
}

// ── Agent Executor ──────────────────────────────────────────

/// Agent executor - core LLM loop
pub struct AgentExecutor<'a> {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: &'a AgentConfig,
}

impl<'a> AgentExecutor<'a> {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: &'a AgentConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

    /// Execute agent loop - pure function with default options
    pub async fn execute(&self, messages: Vec<ChatMessage>) -> Result<ExecutionResult, AgentError> {
        self.execute_with_options(messages, &ExecutorOptions::new())
            .await
    }

    /// Execute with options (cost calculation, log redaction)
    pub async fn execute_with_options(
        &self,
        messages: Vec<ChatMessage>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, AgentError> {
        // No streaming - pass None for event sender
        self.execute_internal(messages, None, options).await
    }

    /// Execute with streaming - sends events to provided channel
    pub async fn execute_stream(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<ExecutionResult, AgentError> {
        self.execute_stream_with_options(messages, event_tx, &ExecutorOptions::new())
            .await
    }

    /// Execute with streaming and options
    pub async fn execute_stream_with_options(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, AgentError> {
        // With streaming - pass Some for event sender
        self.execute_internal(messages, Some(event_tx), options)
            .await
    }

    /// Unified internal implementation for both streaming and non-streaming.
    ///
    /// If `event_tx` is Some, events are forwarded to the channel.
    /// If `event_tx` is None, streaming is disabled (non-streaming mode).
    async fn execute_internal(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: Option<mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, AgentError> {
        let mut state = ExecutionState::new(messages);
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("[Executor] iteration {}", iteration);

            let outcome = self
                .process_iteration(
                    iteration,
                    &mut state,
                    &executor,
                    &request_handler,
                    event_tx.as_ref(),
                    options,
                )
                .await?;

            match outcome {
                IterationOutcome::FinalResponse {
                    content,
                    reasoning_content,
                } => {
                    // Send Done event if streaming
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::Done).await;
                    }
                    return Ok(state.into_result(content, reasoning_content));
                }
                IterationOutcome::ContinueWithTools => {
                    // Continue to next iteration
                }
                IterationOutcome::MaxIterationsReached => {
                    // Send Done event if streaming
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::Done).await;
                    }
                    return Ok(state.into_result(DEFAULT_MAX_ITERATIONS.to_string(), None));
                }
            }
        }

        // This shouldn't be reached due to MaxIterationsReached, but just in case
        if let Some(ref tx) = event_tx {
            let _ = tx.send(StreamEvent::Done).await;
        }
        Ok(state.into_result(DEFAULT_MAX_ITERATIONS.to_string(), None))
    }

    /// Process a single iteration of the agent loop.
    ///
    /// Returns the outcome of this iteration:
    /// - FinalResponse if LLM returned content without tool calls
    /// - ContinueWithTools if tool calls were made
    /// - MaxIterationsReached if this was the last allowed iteration
    async fn process_iteration(
        &self,
        iteration: u32,
        state: &mut ExecutionState,
        executor: &ToolExecutor<'_>,
        request_handler: &RequestHandler<'_>,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) -> Result<IterationOutcome, AgentError> {
        // Step 1: Build and send request
        let request = request_handler.build_chat_request(&state.messages);
        let stream_result = request_handler.send_with_retry(request).await?;

        // Step 2: Get response (streaming or non-streaming)
        let response = self
            .get_response(stream_result, event_tx, state, options)
            .await?;

        // Step 3: Log token usage
        Self::log_token_usage(state, &options.pricing, iteration);

        // Step 4: Log response
        Self::log_response(&response, iteration, options.vault_values);

        // Step 4: Check for final response (no tool calls)
        if let Some(outcome) = Self::check_final_response(&response) {
            return Ok(outcome);
        }

        // Step 5: Execute tool calls
        self.handle_tool_calls(&response, executor, state).await;

        // Step 6: Check max iterations
        if let Some(outcome) = self.check_max_iterations(iteration) {
            return Ok(outcome);
        }

        Ok(IterationOutcome::ContinueWithTools)
    }

    // ── Internal Helpers ────────────────────────────────────

    /// Log token usage if available
    fn log_token_usage(state: &ExecutionState, pricing: &Option<ModelPricing>, iteration: u32) {
        if let Some(ref usage) = state.total_usage {
            let currency = pricing
                .as_ref()
                .map(|p| p.currency.as_str())
                .unwrap_or("USD");
            info!(
                "[Token] iter={} {}",
                iteration,
                crate::token_tracker::format_request_stats(
                    usage,
                    state.total_cost,
                    currency,
                    pricing.as_ref()
                )
            );
        }
    }

    /// Log LLM response with optional redaction
    fn log_response(response: &ChatResponse, iteration: u32, vault_values: &[String]) {
        // Log reasoning if present
        if let Some(ref reasoning) = response.reasoning_content {
            if !reasoning.is_empty() {
                let safe = redact_secrets(reasoning, vault_values);
                debug!("[Executor] Reasoning (iter {}): {}", iteration, safe);
            }
        }

        // Log content if present
        if let Some(ref content) = response.content {
            if !content.is_empty() {
                let safe = redact_secrets(content, vault_values);
                info!("[Executor] Response (iter {}): {}", iteration, safe);
            }
        }

        // Log tool call info
        info!(
            "[Executor] iter {} has_tool_calls={}, tool_count={}",
            iteration,
            response.has_tool_calls(),
            response.tool_calls.len()
        );
    }

    /// Execute tool calls and update state
    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        state: &mut ExecutionState,
    ) {
        if response.tool_calls.is_empty() {
            if let Some(ref c) = response.content {
                state.messages.push(ChatMessage::assistant(c));
            }
            return;
        }

        info!(
            "[Executor] Executing {} tool call(s): {}",
            response.tool_calls.len(),
            response
                .tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Add assistant message with tool calls
        state.messages.push(ChatMessage::assistant_with_tools(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        // Execute tool calls in parallel
        let futures: Vec<_> = response
            .tool_calls
            .iter()
            .map(|tc| async move {
                let start = std::time::Instant::now();
                let result = executor.execute_one(tc).await;
                (tc, result, start.elapsed())
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        for (tool_call, result, duration) in results {
            let tool_name = tool_call.function.name.clone();
            debug!(
                "[Executor] Tool {} -> done ({}ms)",
                tool_name,
                duration.as_millis()
            );

            state.tools_used.push(tool_name.clone());
            state.messages.push(ChatMessage::tool_result(
                tool_call.id.clone(),
                tool_name,
                result.output,
            ));
        }
    }

    // ── Helper Functions for process_iteration ──────────────

    /// Get response from stream, optionally forwarding events to channel
    async fn get_response(
        &self,
        stream_result: ChatStream,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        state: &mut ExecutionState,
        options: &ExecutorOptions<'_>,
    ) -> Result<ChatResponse, AgentError> {
        let response = if let Some(tx) = event_tx {
            // Streaming mode: forward events to channel
            debug!("[Executor] Starting streaming mode, creating event stream");
            let (mut event_stream, response_future) = stream::stream_events(stream_result);

            let mut event_count = 0usize;
            debug!("[Executor] Waiting for events from stream...");
            while let Some(event) = event_stream.next().await {
                event_count += 1;
                if event_count == 1 {
                    debug!("[Executor] Received first event from LLM stream");
                }
                let _ = tx.send(event).await;
            }
            debug!(
                "[Executor] Event stream ended, total events: {}, awaiting response future",
                event_count
            );

            response_future.await?
        } else {
            // Non-streaming mode: collect directly
            stream::collect_stream_response(stream_result).await?
        };

        // Accumulate token usage and cost
        if let Some(ref usage) = response.token_usage() {
            state.accumulate_usage(usage, options.pricing.as_ref());
            Self::send_token_stats_event(state, event_tx, options).await;
        }

        Ok(response)
    }

    /// Send token stats event if streaming
    async fn send_token_stats_event(
        state: &ExecutionState,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) {
        if let Some(tx) = event_tx {
            if let Some(ref total_usage) = state.total_usage {
                let currency = options
                    .pricing
                    .as_ref()
                    .map(|p| p.currency.as_str())
                    .unwrap_or("USD");
                let _ = tx
                    .send(StreamEvent::TokenStats {
                        input_tokens: total_usage.input_tokens,
                        output_tokens: total_usage.output_tokens,
                        total_tokens: total_usage.total_tokens,
                        cost: state.total_cost,
                        currency: currency.to_string(),
                    })
                    .await;
            }
        }
    }

    /// Check if response is final (no tool calls)
    fn check_final_response(response: &ChatResponse) -> Option<IterationOutcome> {
        if !response.has_tool_calls() {
            info!("[Executor] No tool calls, returning final response");
            return Some(IterationOutcome::FinalResponse {
                content: response
                    .content
                    .clone()
                    .unwrap_or_else(|| DEFAULT_NO_RESPONSE.to_string()),
                reasoning_content: response.reasoning_content.clone(),
            });
        }
        None
    }

    /// Check if max iterations reached
    fn check_max_iterations(&self, iteration: u32) -> Option<IterationOutcome> {
        if iteration >= self.config.max_iterations {
            info!(
                "[Executor] Max iterations ({}) reached",
                self.config.max_iterations
            );
            return Some(IterationOutcome::MaxIterationsReached);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_pricing_calculate_cost() {
        let pricing = ModelPricing::new(3.0, 15.0, "USD");

        // 1000 input tokens, 500 output tokens
        let cost = pricing.calculate_cost(1000, 500);
        // Expected: (1000 * 3 / 1_000_000) + (500 * 15 / 1_000_000)
        // = 0.003 + 0.0075 = 0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn test_execution_state_accumulate_usage() {
        let mut state = ExecutionState::new(vec![]);
        let pricing = ModelPricing::new(1.0, 2.0, "USD");

        let usage1 = TokenUsage::new(100, 50);
        state.accumulate_usage(&usage1, Some(&pricing));

        assert_eq!(state.total_usage.as_ref().unwrap().input_tokens, 100);
        assert!((state.total_cost - 0.0002).abs() < 0.00001);

        let usage2 = TokenUsage::new(200, 100);
        state.accumulate_usage(&usage2, Some(&pricing));

        assert_eq!(state.total_usage.as_ref().unwrap().input_tokens, 300);
        assert_eq!(state.total_usage.as_ref().unwrap().output_tokens, 150);
        // Total cost: (100+200) * 1.0/1M + (50+100) * 2.0/1M = 0.0003 + 0.0003 = 0.0006
        assert!((state.total_cost - 0.0006).abs() < 0.00001);
    }

    #[test]
    fn test_execution_state_no_pricing() {
        let mut state = ExecutionState::new(vec![]);
        let usage = TokenUsage::new(100, 50);

        state.accumulate_usage(&usage, None);

        assert_eq!(state.total_usage.as_ref().unwrap().input_tokens, 100);
        assert_eq!(state.total_cost, 0.0);
    }

    #[test]
    fn test_executor_options_builder() {
        let pricing = ModelPricing::new(1.0, 2.0, "USD");
        let vault = vec!["secret".to_string()];

        let opts = ExecutorOptions::new()
            .with_pricing(pricing)
            .with_vault_values(&vault);

        assert!(opts.pricing.is_some());
        assert_eq!(opts.vault_values.len(), 1);
    }
}
