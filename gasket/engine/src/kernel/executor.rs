//! Kernel executor: the pure LLM iteration loop.
//!
//! Extracted from agent/execution/executor.rs. Uses `KernelConfig` instead
//! of `AgentConfig` and `KernelError` instead of `AgentError`.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, instrument, warn};

use super::context::KernelConfig;
use super::error::KernelError;
use super::stream::{self, StreamEvent};
use crate::token_tracker::{ModelPricing, TokenTracker, TokenUsage};
use crate::tools::{SubagentSpawner, ToolContext, ToolRegistry};
use crate::vault::redact_secrets;
use gasket_providers::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ProviderError, ThinkingConfig,
    ToolCall,
};

/// Default response when no content is available
const DEFAULT_NO_RESPONSE: &str = "I've completed processing but have no response to give.";
/// Default response when max iterations reached
const DEFAULT_MAX_ITERATIONS: &str = "Maximum iterations reached.";
/// Maximum retries for transient provider errors.
const MAX_RETRIES: u32 = 3;

// ─────────────────────────────────────────────────────────────────────────────
// ToolExecutor
// ─────────────────────────────────────────────────────────────────────────────

/// Result of executing a single tool call
pub struct ToolCallResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output: String,
}

/// Executes tool calls against a `ToolRegistry`.
pub struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    max_result_chars: usize,
}

impl<'a> ToolExecutor<'a> {
    pub fn new(registry: &'a ToolRegistry, max_result_chars: usize) -> Self {
        Self {
            registry,
            max_result_chars,
        }
    }

    #[instrument(name = "kernel.execute_tool", skip_all, fields(tool = %tool_call.function.name))]
    pub async fn execute_one(&self, tool_call: &ToolCall, ctx: &ToolContext) -> ToolCallResult {
        info!(
            "Tool call: {}({:?})",
            tool_call.function.name, tool_call.function.arguments
        );

        let start = Instant::now();
        let result = self
            .registry
            .execute(
                &tool_call.function.name,
                tool_call.function.arguments.clone(),
                ctx,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e));
        let elapsed = start.elapsed();

        match &result {
            Ok(output) => {
                debug!(
                    tool = %tool_call.function.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    output_len = output.len(),
                    "Tool completed"
                );
            }
            Err(e) => {
                warn!(
                    tool = %tool_call.function.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Tool error"
                );
            }
        }

        let mut result_str = match result {
            Ok(r) => r,
            Err(e) => format!("Error: {}", e),
        };

        if self.max_result_chars > 0 && result_str.len() > self.max_result_chars {
            let mut end = self.max_result_chars;
            while !result_str.is_char_boundary(end) {
                end -= 1;
            }
            result_str.truncate(end);
            result_str.push_str("\n\n[... truncated]");
        }

        ToolCallResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.function.name.clone(),
            output: result_str,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RequestHandler
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for LLM requests with retry support.
pub struct RequestHandler<'a> {
    provider: &'a Arc<dyn LlmProvider>,
    tools: &'a ToolRegistry,
    config: &'a KernelConfig,
}

impl<'a> RequestHandler<'a> {
    pub fn new(
        provider: &'a Arc<dyn LlmProvider>,
        tools: &'a ToolRegistry,
        config: &'a KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

    pub fn build_chat_request(&self, messages: &[ChatMessage]) -> ChatRequest {
        ChatRequest {
            model: self.config.model.clone(),
            messages: messages.to_vec(),
            tools: Some(self.tools.get_definitions()),
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            thinking: if self.config.thinking_enabled {
                Some(ThinkingConfig::enabled())
            } else {
                None
            },
        }
    }

    /// Determine if an error is retryable.
    #[allow(dead_code)]
    fn is_retryable_error(error: &anyhow::Error) -> bool {
        if let Some(provider_err) = error.downcast_ref::<ProviderError>() {
            return provider_err.is_retryable();
        }

        let error_str = error.to_string().to_lowercase();
        let patterns = [
            "connection refused",
            "connection reset",
            "connection timed out",
            "timed out",
            "timeout",
            "dns error",
            "name resolution failed",
            "no route to host",
            "network unreachable",
            "broken pipe",
            "unexpected eof",
            "ssl error",
            "tls error",
            "certificate",
            "hyper::error",
        ];

        for pattern in &patterns {
            if error_str.contains(pattern) {
                return true;
            }
        }

        false
    }

    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream> {
        let mut retries = 0u32;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(provider_err) => {
                    let e = anyhow::anyhow!("{}", provider_err);
                    if !provider_err.is_retryable() {
                        return Err(e.context("Provider request failed (non-retryable)"));
                    }

                    if retries >= MAX_RETRIES {
                        return Err(e.context("Provider request failed after retries"));
                    }
                    retries += 1;
                    warn!(
                        "Provider error (retryable): {}. Retrying {}/{}",
                        e, retries, MAX_RETRIES
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2_u64.pow(retries))).await;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExecutorOptions
// ─────────────────────────────────────────────────────────────────────────────

/// Options for executor behavior
#[derive(Default)]
pub struct ExecutorOptions<'a> {
    pub pricing: Option<ModelPricing>,
    pub vault_values: &'a [String],
    pub token_tracker: Option<Arc<TokenTracker>>,
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

    pub fn with_token_tracker(mut self, tracker: Arc<TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExecutionResult
// ─────────────────────────────────────────────────────────────────────────────

/// Agent execution result
#[derive(Debug)]
pub struct ExecutionResult {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub tools_used: Vec<String>,
    pub token_usage: Option<gasket_types::TokenUsage>,
    pub cost: f64,
}

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

/// Result of a single LLM iteration
enum IterationOutcome {
    FinalResponse {
        content: String,
        reasoning_content: Option<String>,
    },
    ContinueWithTools,
    MaxIterationsReached,
}

// ─────────────────────────────────────────────────────────────────────────────
// AgentExecutor
// ─────────────────────────────────────────────────────────────────────────────

/// Agent executor - core LLM loop
pub struct AgentExecutor<'a> {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: &'a KernelConfig,
    spawner: Option<Arc<dyn SubagentSpawner>>,
}

impl<'a> AgentExecutor<'a> {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: &'a KernelConfig,
    ) -> Self {
        Self::with_spawner(provider, tools, config, None)
    }

    pub fn with_spawner(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: &'a KernelConfig,
        spawner: Option<Arc<dyn SubagentSpawner>>,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            spawner,
        }
    }

    pub async fn execute(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<ExecutionResult, KernelError> {
        self.execute_with_options(messages, &ExecutorOptions::new())
            .await
    }

    pub async fn execute_with_options(
        &self,
        messages: Vec<ChatMessage>,
        options: &ExecutorOptions<'_>,
    ) -> Result<ExecutionResult, KernelError> {
        self.execute_internal(messages, None, options).await
    }

    pub async fn execute_stream(
        &self,
        messages: Vec<ChatMessage>,
        event_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<ExecutionResult, KernelError> {
        self.execute_stream_with_options(messages, event_tx, &ExecutorOptions::new())
            .await
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
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("[Kernel] iteration {}", iteration);

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
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::Done).await;
                    }
                    return Ok(state.into_result(content, reasoning_content));
                }
                IterationOutcome::ContinueWithTools => {}
                IterationOutcome::MaxIterationsReached => {
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(StreamEvent::Done).await;
                    }
                    return Ok(state.into_result(DEFAULT_MAX_ITERATIONS.to_string(), None));
                }
            }
        }

        if let Some(ref tx) = event_tx {
            let _ = tx.send(StreamEvent::Done).await;
        }
        Ok(state.into_result(DEFAULT_MAX_ITERATIONS.to_string(), None))
    }

    async fn process_iteration(
        &self,
        iteration: u32,
        state: &mut ExecutionState,
        executor: &ToolExecutor<'_>,
        request_handler: &RequestHandler<'_>,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) -> Result<IterationOutcome, KernelError> {
        let request = request_handler.build_chat_request(&state.messages);
        let stream_result = request_handler
            .send_with_retry(request)
            .await
            .map_err(|e| KernelError::Provider(e.to_string()))?;

        let response = self
            .get_response(stream_result, event_tx, state, options)
            .await?;

        Self::log_token_usage(state, &options.pricing, iteration);
        Self::log_response(&response, iteration, options.vault_values);

        if let Some(outcome) = Self::check_final_response(&response) {
            return Ok(outcome);
        }

        self.handle_tool_calls(&response, executor, state, event_tx, options)
            .await;

        if let Some(outcome) = self.check_max_iterations(iteration) {
            return Ok(outcome);
        }

        Ok(IterationOutcome::ContinueWithTools)
    }

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

    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        state: &mut ExecutionState,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        options: &ExecutorOptions<'_>,
    ) {
        if response.tool_calls.is_empty() {
            if let Some(ref c) = response.content {
                state.messages.push(ChatMessage::assistant(c));
            }
            return;
        }

        info!(
            "[Kernel] Executing {} tool call(s): {}",
            response.tool_calls.len(),
            response
                .tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        state.messages.push(ChatMessage::assistant_with_tools(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        let ctx = if let Some(ref spawner) = self.spawner {
            let mut ctx = ToolContext::default().spawner(spawner.clone());
            if let Some(ref tracker) = options.token_tracker {
                ctx = ctx.token_tracker(tracker.clone());
            }
            ctx
        } else {
            let mut ctx = ToolContext::default();
            if let Some(ref tracker) = options.token_tracker {
                ctx = ctx.token_tracker(tracker.clone());
            }
            ctx
        };

        let futures: Vec<_> = response
            .tool_calls
            .iter()
            .map(|tc| {
                let ctx = &ctx;
                async move {
                    let start = std::time::Instant::now();
                    let result = executor.execute_one(tc, ctx).await;
                    (tc, result, start.elapsed())
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        for (tool_call, result, duration) in results {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.to_string();

            debug!(
                "[Kernel] Tool {} -> done ({}ms)",
                tool_name,
                duration.as_millis()
            );

            if let Some(tx) = event_tx {
                let _ = tx
                    .send(StreamEvent::ToolStart {
                        name: tool_name.clone(),
                        arguments: Some(tool_args.clone()),
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::ToolEnd {
                        name: tool_name.clone(),
                        output: result.output.clone(),
                    })
                    .await;
            }

            state.tools_used.push(tool_name.clone());
            state.messages.push(ChatMessage::tool_result(
                tool_call.id.clone(),
                tool_name,
                result.output,
            ));
        }
    }

    async fn get_response(
        &self,
        stream_result: ChatStream,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        state: &mut ExecutionState,
        options: &ExecutorOptions<'_>,
    ) -> Result<ChatResponse, KernelError> {
        let response = if let Some(tx) = event_tx {
            debug!("[Kernel] Starting streaming mode");
            let (mut event_stream, response_future) = stream::stream_events(stream_result);

            let mut event_count = 0usize;
            while let Some(event) = event_stream.next().await {
                event_count += 1;
                if event_count == 1 {
                    debug!("[Kernel] Received first event from LLM stream");
                }
                if tx.send(event).await.is_err() {
                    debug!(
                        "[Kernel] Channel closed after {} events, client disconnected",
                        event_count
                    );
                    break;
                }
            }
            debug!("[Kernel] Event stream ended, total events: {}", event_count);

            response_future
                .await
                .map_err(|e| KernelError::Provider(e.to_string()))?
        } else {
            stream::collect_stream_response(stream_result)
                .await
                .map_err(|e| KernelError::Provider(e.to_string()))?
        };

        if let Some(ref api_usage) = response.usage {
            let usage = gasket_types::TokenUsage::from_api_fields(
                api_usage.input_tokens,
                api_usage.output_tokens,
            );
            state.accumulate_usage(&usage, options.pricing.as_ref());
            Self::send_token_stats_event(state, event_tx, options).await;
        }

        Ok(response)
    }

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

    fn check_final_response(response: &ChatResponse) -> Option<IterationOutcome> {
        if !response.has_tool_calls() {
            info!("[Kernel] No tool calls, returning final response");
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

    fn check_max_iterations(&self, iteration: u32) -> Option<IterationOutcome> {
        if iteration >= self.config.max_iterations {
            info!(
                "[Kernel] Max iterations ({}) reached",
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
    use crate::tools::{Tool, ToolError, ToolResult as TResult};
    use async_trait::async_trait;
    use serde_json::Value;

    #[test]
    fn test_model_pricing_calculate_cost() {
        let pricing = ModelPricing::new(3.0, 15.0, "USD");
        let cost = pricing.calculate_cost(1000, 500);
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn test_execution_state_accumulate_usage() {
        let mut state = ExecutionState::new(vec![]);
        let pricing = ModelPricing::new(1.0, 2.0, "USD");

        let usage1 = gasket_types::TokenUsage::new(100, 50);
        state.accumulate_usage(&usage1, Some(&pricing));
        assert_eq!(state.total_usage.as_ref().unwrap().input_tokens, 100);
        assert!((state.total_cost - 0.0002).abs() < 0.00001);

        let usage2 = gasket_types::TokenUsage::new(200, 100);
        state.accumulate_usage(&usage2, Some(&pricing));
        assert_eq!(state.total_usage.as_ref().unwrap().input_tokens, 300);
        assert_eq!(state.total_usage.as_ref().unwrap().output_tokens, 150);
        assert!((state.total_cost - 0.0006).abs() < 0.00001);
    }

    #[test]
    fn test_execution_state_no_pricing() {
        let mut state = ExecutionState::new(vec![]);
        let usage = gasket_types::TokenUsage::new(100, 50);
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

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes back the input"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, args: Value, _ctx: &ToolContext) -> TResult {
            Ok(args.to_string())
        }
    }

    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> TResult {
            Err(ToolError::ExecutionError("boom".to_string()))
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(FailTool));
        reg
    }

    #[tokio::test]
    async fn test_execute_one_success() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("call_1", "echo", serde_json::json!({"msg": "hi"}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.tool_name, "echo");
        assert!(result.output.contains("hi"));
    }

    #[tokio::test]
    async fn test_execute_one_failure() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("call_2", "fail", serde_json::json!({}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 10);

        let tc = ToolCall::new(
            "c1",
            "echo",
            serde_json::json!({"long": "abcdefghijklmnopqrstuvwxyz"}),
        );
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.len() <= 10 + "\n\n[... truncated]".len());
        assert!(result.output.ends_with("[... truncated]"));
    }

    #[tokio::test]
    async fn test_not_found_tool() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("c1", "nonexistent", serde_json::json!({}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation_multibyte_utf8() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 10);

        let tc = ToolCall::new(
            "c1",
            "echo",
            serde_json::json!({"text": "你好世界测试数据更多内容"}),
        );
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.ends_with("[... truncated]"));
        assert!(result.output.is_char_boundary(result.output.len()));
    }

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    #[test]
    fn test_is_retryable_error_network() {
        let err = anyhow::anyhow!("connection timed out");
        assert!(RequestHandler::is_retryable_error(&err));

        let err = anyhow::anyhow!("dns error: name resolution failed");
        assert!(RequestHandler::is_retryable_error(&err));
    }

    #[test]
    fn test_kernel_config_builder() {
        let config = KernelConfig::new("test-model".to_string())
            .with_max_iterations(10)
            .with_temperature(0.5)
            .with_max_tokens(4096)
            .with_thinking(true);

        assert_eq!(config.model, "test-model");
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.max_tokens, 4096);
        assert!(config.thinking_enabled);
    }
}
