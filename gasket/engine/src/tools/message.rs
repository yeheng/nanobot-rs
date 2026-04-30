//! Message tool for sending messages to specific channels

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::channels::ChannelType;
use crate::channels::OutboundMessage;

/// Message tool for sending messages to specific channels.
///
/// Uses the global broker to route messages. Zero-size type — no state needed.
pub struct MessageTool;

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
                    "enum": ["telegram", "discord", "slack", "dingtalk", "feishu", "websocket", "cli"]
                },
                "chat_id": {
                    "type": "string",
                    "description": "Target chat ID (e.g., '123456' for Telegram, 'general' for Slack channel). Use '*' to broadcast to all connected WebSocket clients."
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

    async fn execute(&self, params: Value, ctx: &ToolContext) -> ToolResult {
        let params: MessageParams = serde_json::from_value(params)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        debug!(
            "Message tool invoked: channel={}, chat_id={}",
            params.channel, params.chat_id
        );

        let channel_name = params.channel.to_string();
        let message = if params.chat_id == "*" {
            OutboundMessage::broadcast(params.channel, params.content.clone())
        } else {
            OutboundMessage::new(
                params.channel,
                params.chat_id.clone(),
                params.content.clone(),
            )
        };

        ctx.outbound_tx
            .send(message)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Outbound channel closed: {}", e)))?;

        Ok(format!(
            "Message sent successfully to {}:{}",
            channel_name, params.chat_id
        ).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_message_tool_creation() {
        let tool = MessageTool;
        assert_eq!(tool.name(), "send_message");
        assert!(tool.description().contains("Send a message"));
    }

    #[tokio::test]
    async fn test_message_tool_parameters() {
        let tool = MessageTool;
        let params = tool.parameters();
        assert!(params["properties"]["channel"].is_object());
        assert!(params["properties"]["chat_id"].is_object());
        assert!(params["properties"]["content"].is_object());
    }

    #[tokio::test]
    async fn test_invalid_parameters() {
        let tool = MessageTool;
        let params = serde_json::json!({
            "channel": "telegram"
            // Missing chat_id and content
        });
        let result = tool.execute(params, &ToolContext::default()).await;
        assert!(result.is_err());
    }
}
