//! Spawn tool for subagent execution with synchronous blocking and streaming output
//!
//! This tool spawns a subagent and blocks until completion, streaming events
//! to the WebSocket/channel in real-time. This ensures the main agent waits
//! for results instead of using fire-and-forget semantics.
//!
//! When a `synthesis_callback` is present in the context, the tool operates in
//! non-blocking mode: it spawns the subagent, starts background event forwarding
//! and aggregation, then returns immediately.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::{format_subagent_response, spawn_common, Tool, ToolContext, ToolError, ToolResult};

pub struct SpawnTool;

impl SpawnTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct SpawnArgs {
    task: String,
    model_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to execute a task synchronously with real-time streaming output. \
         The main agent blocks until the subagent completes and returns the result. \
         Use this for tasks that need immediate results with progress feedback."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute"
                },
                "model_id": {
                    "type": "string",
                    "description": "Optional model profile ID to use for this subagent. If not specified, uses the default model."
                }
            },
            "required": ["task"]
        })
    }

    #[instrument(name = "tool.spawn", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let args: SpawnArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        if args.task.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description cannot be empty".to_string(),
            ));
        }

        let spawner = ctx.spawner.as_ref().ok_or_else(|| {
            ToolError::ExecutionError(
                "Subagent spawning is not available in this context (no spawner configured)"
                    .to_string(),
            )
        })?;

        info!("[Spawn] Starting subagent for task: {}", args.task);

        // Spawn subagent via the trait with streaming events
        let (subagent_id, event_rx, result_rx, subagent_cancel_token) = spawner
            .spawn_with_stream(args.task.clone(), args.model_id.clone(), ctx, None)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e)))?;

        // ── Non-blocking mode: synthesis_callback present ──────────────
        if let Some(callback) = ctx.synthesis_callback.clone() {
            let session_key = ctx.session_key.clone();
            let outbound_tx = ctx.outbound_tx.clone();

            // Start background event forwarding
            let _forward_handle = spawn_common::spawn_event_forwarder(
                subagent_id.clone(),
                event_rx,
                session_key.clone(),
                outbound_tx.clone(),
            );

            // Send startup events synchronously (before kernel sends Done)
            spawn_common::send_startup_events(
                &session_key,
                &outbound_tx,
                1,
                &[(subagent_id.clone(), args.task.clone(), 0)],
            )
            .await;

            // Launch background aggregation
            let cancel_token = tokio_util::sync::CancellationToken::new();
            if let Some(ref cancel) = ctx.aggregator_cancel {
                cancel.swap_and_cancel_old(cancel_token.clone());
            }
            spawn_common::spawn_aggregator(
                vec![result_rx],
                vec![subagent_id],
                vec![0],
                vec![subagent_cancel_token],
                callback,
                cancel_token,
                spawn_common::AggregatorContext {
                    session_key,
                    outbound_tx,
                    ws_summary_limit: ctx.ws_summary_limit,
                },
            );

            return Ok("Subagent task dispatched. Results will stream in real-time.".to_string());
        }

        // ── Blocking mode: no synthesis_callback ───────────────────────
        let session_key = ctx.session_key.clone();
        let outbound_tx = ctx.outbound_tx.clone();

        // Notify frontend that subagent has started
        let _ = outbound_tx
            .send(gasket_types::events::OutboundMessage::with_ws_message(
                session_key.channel.clone(),
                session_key.chat_id.clone(),
                gasket_types::events::ChatEvent::subagent_started(
                    subagent_id.clone(),
                    args.task.clone(),
                    0,
                ),
            ))
            .await;

        // Forward subagent events via the shared helper
        let forward_handle = spawn_common::spawn_event_forwarder(
            subagent_id.clone(),
            event_rx,
            session_key.clone(),
            outbound_tx.clone(),
        );

        let result = result_rx.await.map_err(|e| {
            ToolError::ExecutionError(format!("Subagent result channel closed: {}", e))
        })?;

        // Notify frontend that subagent has completed
        let summary = if ctx.ws_summary_limit == 0 {
            result.response.content.clone()
        } else {
            result
                .response
                .content
                .chars()
                .take(ctx.ws_summary_limit)
                .collect::<String>()
        };
        let _ = ctx
            .outbound_tx
            .send(gasket_types::events::OutboundMessage::with_ws_message(
                ctx.session_key.channel.clone(),
                ctx.session_key.chat_id.clone(),
                gasket_types::events::ChatEvent::subagent_completed(
                    subagent_id,
                    0,
                    summary,
                    result.response.tools_used.len() as u32,
                ),
            ))
            .await;

        // Ensure event forwarding completes (or channel is closed)
        let _ = forward_handle.await;

        Ok(format_subagent_response(&result))
    }
}
