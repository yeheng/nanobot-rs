//! Message tool for sending messages to specific channels

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::bus::{MessageBus, OutboundMessage};

/// Message tool for sending messages to specific channels
pub struct MessageTool {
    bus: std::sync::Arc<MessageBus>,
}

impl MessageTool {
    /// Create a new message tool
    pub fn new(bus: std::sync::Arc<MessageBus>) -> Self {
        Self { bus }
    }
}

#[derive(Debug, Deserialize)]
struct MessageParams {
    /// Target channel (e.g., "telegram", "discord", "slack")
    channel: String,

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
                    "description": "Target channel (e.g., 'telegram', 'discord', 'slack', 'email')",
                    "enum": ["telegram", "discord", "slack", "email"]
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

    async fn execute(&self, params: Value) -> ToolResult {
        let params: MessageParams = serde_json::from_value(params)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Create outbound message
        let message = OutboundMessage {
            channel: params.channel.clone(),
            chat_id: params.chat_id.clone(),
            content: params.content.clone(),
            metadata: Default::default(),
        };

        // Send via message bus
        self.bus.publish_outbound(message).await;

        Ok(format!(
            "Message sent successfully to {}:{}",
            params.channel, params.chat_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;

    #[tokio::test]
    async fn test_message_tool_creation() {
        let bus = std::sync::Arc::new(MessageBus::new(10));
        let tool = MessageTool::new(bus);

        assert_eq!(tool.name(), "send_message");
        assert!(tool.description().contains("Send a message"));
    }

    #[tokio::test]
    async fn test_message_tool_parameters() {
        let bus = std::sync::Arc::new(MessageBus::new(10));
        let tool = MessageTool::new(bus);

        let params = tool.parameters();
        assert!(params["properties"]["channel"].is_object());
        assert!(params["properties"]["chat_id"].is_object());
        assert!(params["properties"]["content"].is_object());
    }

    #[tokio::test]
    async fn test_send_message() {
        let bus = std::sync::Arc::new(MessageBus::new(10));
        let tool = MessageTool::new(bus.clone());

        let params = serde_json::json!({
            "channel": "telegram",
            "chat_id": "123456",
            "content": "Hello from MessageTool!"
        });

        let result = tool.execute(params).await;
        assert!(result.is_ok());

        let message = result.unwrap();
        assert!(message.contains("Message sent successfully"));
        assert!(message.contains("telegram:123456"));
    }

    #[tokio::test]
    async fn test_invalid_parameters() {
        let bus = std::sync::Arc::new(MessageBus::new(10));
        let tool = MessageTool::new(bus);

        let params = serde_json::json!({
            "channel": "telegram"
            // Missing chat_id and content
        });

        let result = tool.execute(params).await;
        assert!(result.is_err());
    }
}
