//! `ask_user` — synchronously prompt the user for a reply.
//!
//! Registers a slot in the session's `PendingAskRegistry`, emits the prompt
//! as an `OutboundMessage`, then awaits either the answer or the timeout.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::sleep;

use gasket_types::events::OutboundMessage;
use gasket_types::pending_ask::AskError;
use gasket_types::{Tool, ToolContext, ToolError, ToolResult};

const MAX_TIMEOUT_SECS: u64 = 86_400; // 24 hours

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their reply. Returns a JSON \
         object with the user's answer (content, sender_id, channel, \
         timestamp, optional media). Use only when the user's input is \
         genuinely required to proceed."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The question text shown to the user."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECS,
                    "description": "Maximum seconds to wait. Required."
                }
            },
            "required": ["prompt", "timeout_secs"]
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing 'prompt'".into()))?
            .to_string();

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing/invalid 'timeout_secs'".into())
            })?;
        if !(1..=MAX_TIMEOUT_SECS).contains(&timeout_secs) {
            return Err(ToolError::InvalidArguments(format!(
                "'timeout_secs' must be in [1, {}], got {}",
                MAX_TIMEOUT_SECS, timeout_secs
            )));
        }
        let timeout = Duration::from_secs(timeout_secs);

        let registry = ctx.pending_asks.clone().ok_or_else(|| {
            ToolError::ExecutionError(
                "ask_user requires a PendingAskRegistry in ToolContext; this \
                 context does not support user prompting"
                    .into(),
            )
        })?;

        let deadline = Instant::now() + timeout;
        let registration = registry
            .register(ctx.session_key.clone(), prompt.clone(), deadline)
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;
        let ask_id = registration.ask_id;
        let mut answer_rx = registration.answer_rx;

        // Send prompt to the user channel.
        let outbound = OutboundMessage::new(
            ctx.session_key.channel.clone(),
            ctx.session_key.chat_id.clone(),
            prompt,
        );
        if let Err(e) = ctx.outbound_tx.send(outbound).await {
            registry.cancel(&ctx.session_key, ask_id);
            return Err(ToolError::ExecutionError(format!(
                "failed to send prompt: {}",
                e
            )));
        }

        // Await answer or timeout.
        let answer = tokio::select! {
            biased;
            recv = &mut answer_rx => match recv {
                Ok(answer) => Ok(answer),
                Err(_) => Err(AskError::Cancelled),
            },
            _ = sleep(timeout) => {
                registry.cancel(&ctx.session_key, ask_id);
                Err(AskError::Timeout(timeout))
            }
        };

        match answer {
            Ok(a) => serde_json::to_string(&a).map_err(|e| {
                ToolError::ExecutionError(format!("failed to serialize answer: {}", e))
            }),
            Err(e) => Err(ToolError::ExecutionError(e.to_string())),
        }
    }
}

// ── L2 integration tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PendingAskRegistryImpl;
    use gasket_types::events::{ChannelType, InboundMessage, SessionKey};
    use gasket_types::pending_ask::PendingAskRegistry;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn ctx_for_test(
        registry: Arc<dyn PendingAskRegistry>,
    ) -> (ToolContext, mpsc::Receiver<OutboundMessage>) {
        let (tx, rx) = mpsc::channel::<OutboundMessage>(8);
        let ctx = ToolContext::default()
            .session_key(SessionKey::new(ChannelType::Cli, "test"))
            .outbound_tx(tx)
            .pending_asks(registry);
        (ctx, rx)
    }

    fn dummy_inbound(content: &str, key: &SessionKey) -> InboundMessage {
        InboundMessage {
            channel: key.channel.clone(),
            sender_id: "sender".to_string(),
            chat_id: key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        }
    }

    #[tokio::test]
    async fn happy_path() {
        let registry: Arc<dyn PendingAskRegistry> = Arc::new(PendingAskRegistryImpl::new());
        let (ctx, mut outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 5});

        let task = tokio::spawn(async move { tool.execute(args, &ctx).await });

        let outbound = outbound_rx.recv().await.expect("outbound prompt sent");
        assert_eq!(outbound.content(), "what?");

        registry.try_fulfill(&key, dummy_inbound("answer", &key)).unwrap();

        let result_str = task.await.unwrap().expect("ok result");
        let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();
        assert_eq!(parsed["content"], "answer");
        assert_eq!(parsed["channel"], "cli");
    }

    #[tokio::test]
    async fn timeout_returns_error_and_clears_slot() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, _outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 1});

        let result = tool.execute(args, &ctx).await;
        let err = result.expect_err("expected Timeout error");
        let msg = err.to_string();
        assert!(msg.contains("timed out"), "actual: {msg}");

        // Slot must be empty now — re-registering must succeed.
        let _again = registry
            .register(key.clone(), "q2".into(), Instant::now() + Duration::from_secs(5))
            .expect("slot is free after timeout");
    }

    #[tokio::test]
    async fn cancellation_via_future_drop() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, _outbound_rx) = ctx_for_test(registry.clone());
        let key = ctx.session_key.clone();

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "what?", "timeout_secs": 30});

        let handle = tokio::spawn(async move { tool.execute(args, &ctx).await });
        // Wait for the tool to register, then cancel.
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        // The receiver was dropped along with the future. Registry recovers
        // either via try_fulfill stale-eviction OR via the next register call.
        let _re = registry
            .register(key.clone(), "q2".into(), Instant::now() + Duration::from_secs(5))
            .expect("re-register after future abort");
    }

    #[tokio::test]
    async fn outbound_message_sent_with_prompt() {
        let registry: Arc<dyn PendingAskRegistry> =
            Arc::new(PendingAskRegistryImpl::new());
        let (ctx, mut outbound_rx) = ctx_for_test(registry.clone());

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "abc?", "timeout_secs": 1});
        let _task = tokio::spawn(async move { tool.execute(args, &ctx).await });

        let outbound = outbound_rx.recv().await.expect("prompt was sent");
        assert_eq!(outbound.content(), "abc?");
    }

    #[tokio::test]
    async fn missing_registry_in_context_errors_cleanly() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<OutboundMessage>(1);
        let ctx = ToolContext::default()
            .session_key(SessionKey::new(ChannelType::Cli, "test"))
            .outbound_tx(tx);
        // Note: no .pending_asks() — registry is None.

        let tool = AskUserTool::new();
        let args = serde_json::json!({"prompt": "?", "timeout_secs": 1});
        let err = tool.execute(args, &ctx).await.expect_err("must error");
        assert!(matches!(err, ToolError::ExecutionError(_)));
    }
}
