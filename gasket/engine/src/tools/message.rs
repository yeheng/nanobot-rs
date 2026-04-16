//! Message tool for sending messages to specific channels

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::channels::ChannelType;
use crate::channels::OutboundMessage;

/// Internal dispatch mode for outbound messages.
enum MessageToolMode {
    Direct(mpsc::Sender<OutboundMessage>),
    Broker(Arc<gasket_broker::MemoryBroker>),
}

/// Message tool for sending messages to specific channels.
///
/// Routes through either the Outbound Actor (via mpsc) or the broker
/// (via Topic::Outbound). This decouples the tool from blocking network I/O —
/// the message is enqueued instantly and delivery happens concurrently.
pub struct MessageTool {
    mode: MessageToolMode,
}

impl MessageTool {
    /// Create a new message tool that routes through the outbound mpsc channel.
    pub fn new(outbound_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            mode: MessageToolMode::Direct(outbound_tx),
        }
    }

    /// Create a new message tool backed by the message broker.
    pub fn new_broker(broker: Arc<gasket_broker::MemoryBroker>) -> Self {
        Self {
            mode: MessageToolMode::Broker(broker),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MessageParams {
    /// Target channel (e.g., "telegram", "discord", "slack")
    channel: ChannelType,

    /// Target chat ID
    chat_id: String,

    /// Message content
    content: String,
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a message to a specific channel and chat. Use this to proactively reach out to users or send notifications."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Target channel (e.g., 'telegram', 'discord', 'slack')",
                    "enum": ["telegram", "discord", "slack", "dingtalk", "feishu", "cli"]
                },
                "chat_id": {
                    "type": "string",
                    "description": "Target chat ID (e.g., '123456' for Telegram, 'general' for Slack channel)"
                },
                "content": {
                    "type": "string",
                    "description": "Message content to send (supports Markdown formatting)"
                }
            },
            "required": ["channel", "chat_id", "content"]
        })
    }

    #[instrument(name = "tool.send_message", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> ToolResult {
        let params: MessageParams = serde_json::from_value(params)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Create outbound message
        let channel_name = params.channel.to_string();
        let message = OutboundMessage {
            channel: params.channel,
            chat_id: params.chat_id.clone(),
            content: params.content.clone(),
            metadata: Default::default(),
            trace_id: None,
            ws_message: None,
        };

        // Route through Outbound Actor or broker — enqueue instantly, no network wait.
        match &self.mode {
            MessageToolMode::Direct(tx) => {
                tx.send(message).await.map_err(|e| {
                    ToolError::ExecutionError(format!("Outbound channel closed: {}", e))
                })?;
            }
            MessageToolMode::Broker(broker) => {
                let envelope = gasket_broker::Envelope::new(
                    gasket_broker::Topic::Outbound,
                    gasket_broker::BrokerPayload::Outbound(message),
                );
                broker.publish(envelope).await.map_err(|e| {
                    ToolError::ExecutionError(format!("Broker publish failed: {}", e))
                })?;
            }
        }

        Ok(format!(
            "Message sent successfully to {}:{}",
            channel_name, params.chat_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_tool() -> (MessageTool, mpsc::Receiver<OutboundMessage>) {
        let (tx, rx) = mpsc::channel(16);
        (MessageTool::new(tx), rx)
    }

    #[tokio::test]
    async fn test_message_tool_creation() {
        let (tool, _rx) = make_test_tool();
        assert_eq!(tool.name(), "send_message");
        assert!(tool.description().contains("Send a message"));
    }

    #[tokio::test]
    async fn test_message_tool_parameters() {
        let (tool, _rx) = make_test_tool();
        let params = tool.parameters();
        assert!(params["properties"]["channel"].is_object());
        assert!(params["properties"]["chat_id"].is_object());
        assert!(params["properties"]["content"].is_object());
    }

    #[tokio::test]
    async fn test_invalid_parameters() {
        let (tool, _rx) = make_test_tool();
        let params = serde_json::json!({
            "channel": "telegram"
            // Missing chat_id and content
        });
        let result = tool.execute(params, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_routes_to_outbound_channel() {
        let (tool, mut rx) = make_test_tool();
        let params = serde_json::json!({
            "channel": "telegram",
            "chat_id": "12345",
            "content": "Hello!"
        });
        let result = tool.execute(params, &ToolContext::default()).await;
        assert!(result.is_ok());

        // Verify the message was routed to the outbound channel
        let msg = rx
            .try_recv()
            .expect("should have received outbound message");
        assert_eq!(msg.chat_id, "12345");
        assert_eq!(msg.content, "Hello!");
    }
}
