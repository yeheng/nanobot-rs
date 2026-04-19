//! Kernel executor: the pure LLM iteration loop.
//!
//! Extracted from agent/execution/executor.rs. Uses `KernelConfig` instead
//! of `AgentConfig` and `KernelError` instead of `AgentError`.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, instrument, warn};

use super::context::KernelConfig;
use super::error::KernelError;
use super::stream::{self};
use crate::token_tracker::TokenUsage;
use crate::tools::{SubagentSpawner, ToolContext, ToolRegistry};
use crate::vault::redact_secrets;
use gasket_providers::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ThinkingConfig, ToolCall,
};
use gasket_types::StreamEvent;

/// Default response when no content is available
const DEFAULT_NO_RESPONSE: &str = "I've completed processing but have no response to give.";

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
            let original_len = result_str.len();
            let mut end = self.max_result_chars;
            while !result_str.is_char_boundary(end) {
                end -= 1;
            }
            result_str.truncate(end);
            result_str.push_str(&format!(
                "\n\n[OUTPUT TRUNCATED: original {} chars exceeded limit of {} chars]",
                original_len, self.max_result_chars
            ));
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

    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream> {
        let mut retries = 0u32;
        let max_retries = self.config.max_retries;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(provider_err) => {
                    let e = anyhow::anyhow!("{}", provider_err);
                    if !provider_err.is_retryable() {
                        return Err(e.context("Provider request failed (non-retryable)"));
                    }

                    if retries >= max_retries {
                        return Err(e.context("Provider request failed after retries"));
                    }
                    retries += 1;
                    warn!(
                        "Provider error (retryable): {}. Retrying {}/{}",
                        e, retries, max_retries
                    );
                    // Safe exponential backoff: 2^retries seconds, capped at 15s.
                    // Use shift to avoid u64::pow overflow on high retry counts.
                    let backoff_secs = (1u64 << retries.min(63)).min(15);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
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
            cost: 0.0,
        }
    }
}

/// Token usage ledger — separated from ExecutionState for purity.
///
/// Each iteration returns its `TokenUsage` delta; this ledger accumulates
/// them across the full execution lifecycle.
struct TokenLedger {
    total_usage: Option<TokenUsage>,
}

impl TokenLedger {
    fn new() -> Self {
        Self { total_usage: None }
    }

    fn accumulate(&mut self, usage: &TokenUsage) {
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

// ─────────────────────────────────────────────────────────────────────────────
// AgentExecutor
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel executor - core LLM loop
pub struct KernelExecutor<'a> {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: &'a KernelConfig,
    spawner: Option<Arc<dyn SubagentSpawner>>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}

impl<'a> KernelExecutor<'a> {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: &'a KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            spawner: None,
            token_tracker: None,
        }
    }

    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.spawner = Some(spawner);
        self
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
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
        let request_handler = RequestHandler::new(&self.provider, &self.tools, self.config);
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);

        for iteration in 1..=self.config.max_iterations {
            debug!("[Kernel] iteration {}", iteration);

            let request = request_handler.build_chat_request(&state.messages);
            let stream_result = request_handler
                .send_with_retry(request)
                .await
                .map_err(|e| KernelError::Provider(e.to_string()))?;

            let response = self.get_response(stream_result, event_tx, ledger).await?;

            Self::log_token_usage(ledger, iteration);
            Self::log_response(&response, iteration, options.vault_values);

            if let Some((content, reasoning_content)) = Self::check_final_response(&response) {
                return Ok(state.to_result(content, reasoning_content, ledger));
            }

            self.handle_tool_calls(&response, &executor, state, event_tx)
                .await;

            if iteration >= self.config.max_iterations {
                info!(
                    "[Kernel] Max iterations ({}) reached",
                    self.config.max_iterations
                );
                return Err(KernelError::MaxIterations(self.config.max_iterations));
            }
        }

        Err(KernelError::MaxIterations(self.config.max_iterations))
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

    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        state: &mut ExecutionState,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
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

        let mut ctx = ToolContext::default();
        if let Some(ref spawner) = self.spawner {
            ctx = ctx.spawner(spawner.clone());
        }
        if let Some(ref tracker) = self.token_tracker {
            ctx = ctx.token_tracker(tracker.clone());
        }

        // 并发执行工具调用，每个工具独立发送事件
        let futures: Vec<_> = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(idx, tc)| {
                let tool_call = tc.clone();
                let ctx = ctx.clone();
                let tx = event_tx.cloned();
                async move {
                    let tool_name = tool_call.function.name.clone();
                    let tool_args = tool_call.function.arguments.to_string();

                    // 发送 tool_start 事件
                    if let Some(ref sender) = tx {
                        let _ = sender
                            .send(StreamEvent::tool_start(&tool_name, Some(tool_args)))
                            .await;
                    }

                    let start = std::time::Instant::now();
                    let result = executor.execute_one(&tool_call, &ctx).await;
                    let duration = start.elapsed();

                    debug!(
                        "[Kernel] Tool {} -> done ({}ms)",
                        tool_name,
                        duration.as_millis()
                    );

                    // 发送 tool_end 事件
                    if let Some(ref sender) = tx {
                        let _ = sender
                            .send(StreamEvent::tool_end(
                                &tool_name,
                                Some(result.output.clone()),
                            ))
                            .await;
                    }

                    (idx, tool_call.id, tool_name, result.output)
                }
            })
            .collect();

        let mut results = futures_util::future::join_all(futures).await;
        // 按原始顺序排序，确保消息顺序一致
        results.sort_by_key(|(idx, _, _, _)| *idx);

        for (_, tool_call_id, tool_name, output) in results {
            state.tools_used.push(tool_name.clone());
            state
                .messages
                .push(ChatMessage::tool_result(tool_call_id, tool_name, output));
        }
    }

    async fn get_response(
        &self,
        stream_result: ChatStream,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        ledger: &mut TokenLedger,
    ) -> Result<ChatResponse, KernelError> {
        // Always use the streaming pipeline — "Everything is a Stream".
        // For non-streaming callers, events are silently drained.
        let (mut event_stream, response_future) = stream::stream_events(stream_result);

        if let Some(tx) = event_tx {
            // Forward events to external channel
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
        } else {
            // Non-streaming: silently drain events to drive the stream to completion
            while event_stream.next().await.is_some() {}
        }

        let response = response_future
            .await
            .map_err(|e| KernelError::Provider(e.to_string()))?;

        if let Some(ref api_usage) = response.usage {
            let usage = gasket_types::TokenUsage::from_api_fields(
                api_usage.input_tokens,
                api_usage.output_tokens,
            );
            ledger.accumulate(&usage);
        }

        Ok(response)
    }

    fn check_final_response(response: &ChatResponse) -> Option<(String, Option<String>)> {
        if response.has_tool_calls() {
            return None;
        }
        info!("[Kernel] No tool calls, returning final response");
        let content = response
            .content
            .clone()
            .unwrap_or_else(|| DEFAULT_NO_RESPONSE.to_string());
        Some((content, response.reasoning_content.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolError, ToolResult as TResult};
    use async_trait::async_trait;
    use serde_json::Value;

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
        fn as_any(&self) -> &dyn std::any::Any {
            self
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
        fn as_any(&self) -> &dyn std::any::Any {
            self
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
    fn test_kernel_config_builder() {
        let config = KernelConfig::new("test-model".to_string())
            .with_max_iterations(10)
            .with_max_retries(5)
            .with_temperature(0.5)
            .with_max_tokens(4096)
            .with_thinking(true);

        assert_eq!(config.model, "test-model");
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.max_tokens, 4096);
        assert!(config.thinking_enabled);
    }
}
