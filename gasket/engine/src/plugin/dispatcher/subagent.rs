//! Subagent spawn callback handler.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::plugin::manifest::Permission;
use crate::plugin::rpc::RpcError;

/// Handler for `subagent/spawn` RPC method calls.
///
/// This handler processes subagent spawn requests from scripts by delegating
/// to the SubagentSpawner in the context.
pub struct SubagentSpawnHandler;

/// Request parameters for spawning a subagent.
#[derive(Debug, Deserialize, Serialize)]
struct SpawnRequest {
    /// Task description for the subagent
    task: String,
    /// Optional model profile ID to use
    model_id: Option<String>,
}

/// Response from spawning a subagent.
#[derive(Debug, Serialize)]
struct SpawnResponse {
    /// Subagent session ID
    id: String,
    /// Task that was executed
    task: String,
    /// Response content from the subagent
    content: String,
    /// Model used for execution
    model: Option<String>,
}

#[async_trait]
impl RpcHandler for SubagentSpawnHandler {
    fn method(&self) -> &str {
        "subagent/spawn"
    }

    fn required_permission(&self) -> Permission {
        Permission::SubagentSpawn
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let spawner = ctx.engine.spawner.as_ref().ok_or_else(|| {
            RpcError::internal_error(
                "Subagent spawning is not available in this context (no spawner configured)"
                    .to_string(),
            )
        })?;

        let request: SpawnRequest = serde_json::from_value(params).map_err(|e| {
            RpcError::invalid_params(format!("Failed to parse SpawnRequest: {}", e))
        })?;

        // Use the streaming variant so the frontend receives live
        // thinking/content events instead of a frozen UI.
        let (subagent_id, event_rx, result_rx, _cancel_token) = (*spawner)
            .spawn_with_stream(request.task.clone(), request.model_id.clone())
            .await
            .map_err(|e| RpcError::internal_error(format!("Subagent spawn failed: {}", e)))?;

        // Notify frontend that the subagent has started (matches SpawnTool behavior).
        let _ = ctx
            .engine
            .outbound_tx
            .send(gasket_types::events::OutboundMessage::with_ws_message(
                ctx.engine.session_key.channel.clone(),
                ctx.engine.session_key.chat_id.clone(),
                gasket_types::events::ChatEvent::subagent_started(
                    subagent_id.clone(),
                    request.task.clone(),
                    0,
                ),
            ))
            .await;

        // Forward StreamEvents → ChatEvents via the shared helper used by SpawnTool.
        let _forward_handle = crate::tools::spawn_common::spawn_event_forwarder(
            subagent_id.clone(),
            event_rx,
            ctx.engine.session_key.clone(),
            ctx.engine.outbound_tx.clone(),
        );

        let result = result_rx
            .await
            .map_err(|e| RpcError::internal_error(format!("Subagent result dropped: {}", e)))?;

        let response = SpawnResponse {
            id: result.id,
            task: result.task,
            content: result.response.content,
            model: result.model,
        };

        serde_json::to_value(response)
            .map_err(|e| RpcError::internal_error(format!("Failed to serialize response: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;
    use gasket_types::events::SessionKey;
    use gasket_types::{StreamEvent, SubagentResult, SubagentSpawner};
    use serde_json::json;
    use std::sync::Arc;

    /// Spawner that emits scripted StreamEvents through the streaming channel
    /// and then completes the result oneshot.
    struct MockStreamingSpawner {
        scripted_events: std::sync::Mutex<Vec<StreamEvent>>,
    }

    #[async_trait::async_trait]
    impl SubagentSpawner for MockStreamingSpawner {
        async fn spawn(
            &self,
            _task: String,
            _model_id: Option<String>,
        ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
            unreachable!("streaming handler must call spawn_with_stream, not spawn")
        }

        async fn spawn_with_stream(
            &self,
            task: String,
            model_id: Option<String>,
        ) -> Result<
            (
                String,
                tokio::sync::mpsc::Receiver<StreamEvent>,
                tokio::sync::oneshot::Receiver<SubagentResult>,
                tokio_util::sync::CancellationToken,
            ),
            Box<dyn std::error::Error + Send>,
        > {
            let (event_tx, event_rx) = tokio::sync::mpsc::channel(8);
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let cancel = tokio_util::sync::CancellationToken::new();

            let events: Vec<StreamEvent> = self.scripted_events.lock().unwrap().drain(..).collect();
            let task_clone = task.clone();
            let model_clone = model_id.clone();
            tokio::spawn(async move {
                for ev in events {
                    let _ = event_tx.send(ev).await;
                }
                drop(event_tx);
                let _ = result_tx.send(SubagentResult {
                    id: "mock-streaming".to_string(),
                    task: task_clone,
                    response: gasket_types::SubagentResponse {
                        content: "final-content".to_string(),
                        reasoning_content: None,
                        tools_used: vec![],
                        model: None,
                        token_usage: None,
                        cost: 0.0,
                    },
                    model: model_clone,
                });
            });

            Ok(("mock-streaming".to_string(), event_rx, result_rx, cancel))
        }
    }

    fn ctx_with_streaming_spawner(
        scripted: Vec<StreamEvent>,
    ) -> (
        DispatcherContext,
        tokio::sync::mpsc::Receiver<gasket_types::events::OutboundMessage>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        pub struct MockProvider;
        #[async_trait::async_trait]
        impl gasket_providers::LlmProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            fn default_model(&self) -> &str {
                "mock-model"
            }
            async fn chat(
                &self,
                _request: gasket_providers::ChatRequest,
            ) -> Result<gasket_providers::ChatResponse, gasket_providers::ProviderError>
            {
                Ok(gasket_providers::ChatResponse {
                    content: Some("test".to_string()),
                    tool_calls: vec![],
                    reasoning_content: None,
                    usage: None,
                })
            }
        }

        let ctx = DispatcherContext {
            engine: Arc::new(super::super::EngineHandle {
                session_key: SessionKey::new(
                    gasket_types::events::ChannelType::Telegram,
                    "test-chat",
                ),
                outbound_tx: tx,
                spawner: Some(Arc::new(MockStreamingSpawner {
                    scripted_events: std::sync::Mutex::new(scripted),
                })),
                token_tracker: Arc::new(gasket_types::token_tracker::TokenTracker::unlimited(
                    "USD",
                )),
                tool_registry: Arc::new(ToolRegistry::new()),
                provider: Arc::new(MockProvider),
                pending_asks: None,
            }),
        };
        (ctx, rx)
    }

    #[tokio::test]
    async fn test_subagent_spawn_forwards_stream_events() {
        let scripted = vec![
            StreamEvent::thinking("hello-thinking"),
            StreamEvent::content("hello-content"),
        ];
        let (ctx, mut rx) = ctx_with_streaming_spawner(scripted);

        let handler = SubagentSpawnHandler;
        let params = json!({"task": "demo", "model_id": null});
        let result = handler.handle(params, &ctx).await.expect("handler ok");

        // SpawnResponse JSON shape unchanged
        assert_eq!(result["id"], json!("mock-streaming"));
        assert_eq!(result["content"], json!("final-content"));

        // Drain outbound messages and collect ChatEvent kinds
        use gasket_types::events::{ChatEvent, OutboundPayload};
        let mut kinds: Vec<&'static str> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await {
                Ok(Some(msg)) => {
                    if let OutboundPayload::Stream(ev) = msg.payload {
                        let kind = match ev {
                            ChatEvent::SubagentStarted { .. } => "started",
                            ChatEvent::SubagentThinking { .. } => "thinking",
                            ChatEvent::SubagentContent { .. } => "content",
                            ChatEvent::SubagentToolStart { .. } => "tool_start",
                            ChatEvent::SubagentToolEnd { .. } => "tool_end",
                            _ => continue,
                        };
                        kinds.push(kind);
                    }
                }
                _ => break,
            }
        }
        assert!(
            kinds.contains(&"started"),
            "missing SubagentStarted; got {:?}",
            kinds
        );
        assert!(
            kinds.contains(&"thinking"),
            "missing SubagentThinking; got {:?}",
            kinds
        );
        assert!(
            kinds.contains(&"content"),
            "missing SubagentContent; got {:?}",
            kinds
        );
    }
}
