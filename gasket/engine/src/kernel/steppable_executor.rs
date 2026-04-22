//! Steppable executor — one LLM call + optional tool execution per `step()`.
//!
//! External callers (like `MonitoredRunner`) drive the loop; `KernelExecutor`
//! composes this internally.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::debug;

use crate::kernel::{
    context::RuntimeContext,
    error::KernelError,
    request_handler::RequestHandler,
    stream,
    tool_executor::{ToolCallResult, ToolExecutor},
};
use crate::tools::ToolContext;
use gasket_providers::{ChatMessage, ChatResponse, ChatStream};
use gasket_types::StreamEvent;

/// Result of executing one LLM iteration.
///
/// Returned by `SteppableExecutor::step()` so callers can inspect each turn
/// without owning the full loop.
#[derive(Debug)]
pub struct StepResult {
    pub response: ChatResponse,
    pub tool_results: Vec<ToolCallResult>,
    pub should_continue: bool,
}

pub struct SteppableExecutor {
    ctx: RuntimeContext,
}

impl SteppableExecutor {
    pub fn new(ctx: RuntimeContext) -> Self {
        Self { ctx }
    }

    pub fn with_spawner(mut self, spawner: Arc<dyn crate::tools::SubagentSpawner>) -> Self {
        self.ctx.spawner = Some(spawner);
        self
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.ctx.token_tracker = Some(tracker);
        self
    }

    /// Enable proactive checkpointing via interceptor.
    pub fn with_checkpoint(
        mut self,
        callback: Arc<dyn Fn(usize) -> Option<String> + Send + Sync>,
    ) -> Self {
        self.ctx.checkpoint_callback = Some(callback);
        self
    }

    /// Execute one iteration: LLM call → optional tool calls → return result.
    ///
    /// `messages` is mutated in place (assistant response + tool results appended).
    /// `ledger` accumulates token usage across steps.
    pub async fn step(
        &self,
        messages: &mut Vec<ChatMessage>,
        ledger: &mut crate::kernel::kernel_executor::TokenLedger,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Result<StepResult, KernelError> {
        // Proactive checkpoint injection (before LLM call)
        if let Some(ref cb) = self.ctx.checkpoint_callback {
            if let Some(summary) = cb(messages.len()) {
                debug!("[Steppable] Injecting checkpoint ({} chars)", summary.len());
                messages.push(ChatMessage::system(format!("[Working Memory] {}", summary)));
            }
        }

        let request_handler =
            RequestHandler::new(&self.ctx.provider, &self.ctx.tools, &self.ctx.config);
        let executor = ToolExecutor::new(&self.ctx.tools, self.ctx.config.max_tool_result_chars);

        let request = request_handler.build_chat_request(messages);
        let stream_result = request_handler
            .send_with_retry(request)
            .await
            .map_err(|e| KernelError::Provider(e.to_string()))?;

        let response = self.get_response(stream_result, event_tx, ledger).await?;

        let is_final = !response.has_tool_calls();

        if is_final {
            if let Some(ref content) = response.content {
                messages.push(ChatMessage::assistant(content));
            }
            return Ok(StepResult {
                response,
                tool_results: vec![],
                should_continue: false,
            });
        }

        // Handle tool calls — mutates messages, returns results for progress reporting
        let tool_results = self
            .handle_tool_calls(&response, &executor, messages, event_tx)
            .await;

        Ok(StepResult {
            response,
            tool_results,
            should_continue: true,
        })
    }

    async fn get_response(
        &self,
        stream_result: ChatStream,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        ledger: &mut crate::kernel::kernel_executor::TokenLedger,
    ) -> Result<ChatResponse, KernelError> {
        let (mut event_stream, response_future, _handle) = stream::stream_events(stream_result);

        if let Some(tx) = event_tx {
            let mut event_count = 0usize;
            while let Some(event) = event_stream.next().await {
                event_count += 1;
                if event_count == 1 {
                    debug!("[Steppable] Received first event from LLM stream");
                }
                if tx.send(event).await.is_err() {
                    debug!("[Steppable] Channel closed after {} events", event_count);
                    break;
                }
            }
        } else {
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

    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        messages: &mut Vec<ChatMessage>,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Vec<ToolCallResult> {
        // Note: caller already checked `has_tool_calls()`, so `tool_calls` is non-empty.
        messages.push(ChatMessage::assistant_with_tools(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        let mut ctx = ToolContext::default();
        if let Some(ref spawner) = self.ctx.spawner {
            ctx = ctx.spawner(spawner.clone());
        }
        if let Some(ref tracker) = self.ctx.token_tracker {
            ctx = ctx.token_tracker(tracker.clone());
        }

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

                    if let Some(ref sender) = tx {
                        let _ = sender
                            .send(StreamEvent::tool_start(&tool_name, Some(tool_args)))
                            .await;
                    }

                    let start = std::time::Instant::now();
                    let result = executor.execute_one(&tool_call, &ctx).await;
                    let duration = start.elapsed();

                    debug!(
                        "[Steppable] Tool {} -> done ({}ms)",
                        tool_name,
                        duration.as_millis()
                    );

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
        results.sort_by_key(|(idx, _, _, _)| *idx);

        let mut tool_results = Vec::new();
        for (_, tool_call_id, tool_name, output) in results {
            messages.push(ChatMessage::tool_result(
                tool_call_id.clone(),
                tool_name.clone(),
                output.clone(),
            ));
            tool_results.push(ToolCallResult {
                tool_call_id,
                tool_name,
                output,
            });
        }
        tool_results
    }
}
