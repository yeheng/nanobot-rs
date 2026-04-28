//! Spawn tool for subagent execution with synchronous blocking and streaming output
//!
//! This tool spawns a subagent and blocks until completion, streaming events
//! to the WebSocket/channel in real-time. This ensures the main agent waits
//! for results instead of using fire-and-forget semantics.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::{format_subagent_response, Tool, ToolContext, ToolError, ToolResult};

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

        // Get spawner from context (always present, may be NoopSpawner)
        let spawner = &ctx.spawner;

        info!("[Spawn] Starting subagent for task: {}", args.task);

        // Spawn subagent via the trait with streaming events
        let (subagent_id, mut event_rx, result_rx) = spawner
            .spawn_with_stream(args.task.clone(), args.model_id.clone())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e)))?;

        // Notify frontend that subagent has started
        let session_key = ctx.session_key.clone();
        let outbound_tx = ctx.outbound_tx.clone();
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

        // Forward subagent events to WebSocket in real-time
        let fwd_subagent_id = subagent_id.clone();
        let fwd_session_key = session_key.clone();
        let fwd_outbound_tx = outbound_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                use gasket_types::events::ChatEvent;
                use gasket_types::StreamEventKind;

                let chat_event = match &event.kind {
                    StreamEventKind::Thinking { content } => Some(ChatEvent::subagent_thinking(
                        fwd_subagent_id.clone(),
                        content.as_ref(),
                    )),
                    StreamEventKind::ToolStart { name, arguments } => {
                        Some(ChatEvent::subagent_tool_start(
                            fwd_subagent_id.clone(),
                            name.as_ref(),
                            arguments.as_ref().map(|s| s.to_string()),
                        ))
                    }
                    StreamEventKind::ToolEnd { name, output } => {
                        Some(ChatEvent::subagent_tool_end(
                            fwd_subagent_id.clone(),
                            name.as_ref(),
                            output.as_ref().map(|s| s.to_string()),
                        ))
                    }
                    StreamEventKind::Content { content } => Some(ChatEvent::subagent_content(
                        fwd_subagent_id.clone(),
                        content.as_ref(),
                    )),
                    _ => None,
                };

                if let Some(chat_event) = chat_event {
                    let msg = gasket_types::events::OutboundMessage::with_ws_message(
                        fwd_session_key.channel.clone(),
                        fwd_session_key.chat_id.clone(),
                        chat_event,
                    );
                    let _ = fwd_outbound_tx.send(msg).await;
                }
            }
        });

        let result = result_rx.await.map_err(|e| {
            ToolError::ExecutionError(format!("Subagent result channel closed: {}", e))
        })?;

        // Notify frontend that subagent has completed
        let summary = result
            .response
            .content
            .chars()
            .take(100)
            .collect::<String>();
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
