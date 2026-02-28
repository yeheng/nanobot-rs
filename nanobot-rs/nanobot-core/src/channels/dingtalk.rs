//! DingTalk (钉钉) channel implementation
//!
//! Supports DingTalk robot messaging via webhook and API

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, instrument};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::ChannelType;
use crate::channels::middleware::InboundSender;

/// DingTalk channel configuration
#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    /// Webhook URL (for outgoing messages)
    pub webhook_url: String,

    /// Secret key for signing (optional but recommended)
    pub secret: Option<String>,

    /// Access token (alternative to webhook_url)
    pub access_token: Option<String>,

    /// Allowed users (empty = allow all)
    pub allow_from: Vec<String>,
}

/// DingTalk channel.
///
/// Sends incoming messages through `InboundSender` which applies auth/rate-limit
/// checks before forwarding to the message bus.
pub struct DingTalkChannel {
    config: DingTalkConfig,
    inbound_sender: InboundSender,
    client: Client,
}

impl DingTalkChannel {
    /// Create a new DingTalk channel with an inbound message sender.
    pub fn new(config: DingTalkConfig, inbound_sender: InboundSender) -> Self {
        Self {
            config,
            inbound_sender,
            client: Client::new(),
        }
    }

    /// Generate signed webhook URL with timestamp and sign
    fn get_signed_webhook_url(&self) -> String {
        let base_url = if self.config.webhook_url.is_empty() {
            format!(
                "https://oapi.dingtalk.com/robot/send?access_token={}",
                self.config.access_token.as_deref().unwrap_or("")
            )
        } else {
            self.config.webhook_url.clone()
        };

        if let Some(ref secret) = self.config.secret {
            let timestamp = chrono::Utc::now().timestamp_millis();
            let string_to_sign = format!("{}\n{}", timestamp, secret);

            let mut hmac = Sha256::new();
            hmac.update(string_to_sign.as_bytes());
            let sign = BASE64.encode(hmac.finalize());
            let sign_encoded = urlencoding::encode(&sign);

            format!("{}&timestamp={}&sign={}", base_url, timestamp, sign_encoded)
        } else {
            base_url
        }
    }

    /// Send a text message via webhook
    #[instrument(name = "channel.dingtalk.send_text", skip_all)]
    pub async fn send_text(&self, text: &str) -> anyhow::Result<()> {
        let url = self.get_signed_webhook_url();

        #[derive(Serialize)]
        struct TextMessage {
            msgtype: String,
            text: TextContent,
        }

        #[derive(Serialize)]
        struct TextContent {
            content: String,
        }

        let message = TextMessage {
            msgtype: "text".to_string(),
            text: TextContent {
                content: text.to_string(),
            },
        };

        let response = self.client.post(&url).json(&message).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to send DingTalk message: {} - {}", status, body);
        }

        #[derive(Deserialize)]
        struct DingTalkResponse {
            errcode: i32,
            errmsg: String,
        }

        let result: DingTalkResponse = response.json().await?;
        if result.errcode != 0 {
            anyhow::bail!("DingTalk API error: {} - {}", result.errcode, result.errmsg);
        }

        debug!("Sent DingTalk message: {}", text);
        Ok(())
    }

    /// Send a markdown message via webhook
    #[instrument(name = "channel.dingtalk.send_markdown", skip_all)]
    pub async fn send_markdown(&self, title: &str, text: &str) -> anyhow::Result<()> {
        let url = self.get_signed_webhook_url();

        #[derive(Serialize)]
        struct MarkdownMessage {
            msgtype: String,
            markdown: MarkdownContent,
        }

        #[derive(Serialize)]
        struct MarkdownContent {
            title: String,
            text: String,
        }

        let message = MarkdownMessage {
            msgtype: "markdown".to_string(),
            markdown: MarkdownContent {
                title: title.to_string(),
                text: text.to_string(),
            },
        };

        let response = self.client.post(&url).json(&message).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to send DingTalk markdown: {} - {}", status, body);
        }

        #[derive(Deserialize)]
        struct DingTalkResponse {
            errcode: i32,
            errmsg: String,
        }

        let result: DingTalkResponse = response.json().await?;
        if result.errcode != 0 {
            anyhow::bail!("DingTalk API error: {} - {}", result.errcode, result.errmsg);
        }

        debug!("Sent DingTalk markdown message with title: {}", title);
        Ok(())
    }

    /// Handle incoming callback message (for 2.0 robots with callback mode)
    #[instrument(name = "channel.dingtalk.handle_callback", skip_all)]
    pub async fn handle_callback_message(
        &self,
        message: DingTalkCallbackMessage,
    ) -> anyhow::Result<()> {
        // Check allowlist
        if !self.config.allow_from.is_empty() {
            let sender_id = message.sender_id.clone();
            if !self.config.allow_from.contains(&sender_id) {
                debug!(
                    "Ignoring message from unauthorized DingTalk user: {}",
                    sender_id
                );
                return Ok(());
            }
        }

        // Only handle text messages
        if message.msgtype != "text" {
            debug!("Ignoring non-text DingTalk message: {}", message.msgtype);
            return Ok(());
        }

        let content = message.text.content.clone();
        debug!("Received DingTalk message: {}", content);

        let metadata = serde_json::to_value(&message).ok();

        let inbound = InboundMessage {
            channel: ChannelType::Dingtalk,
            sender_id: message.sender_id.clone(),
            chat_id: message.conversation_id.clone(),
            content,
            media: None,
            metadata,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };

        self.inbound_sender.send(inbound).await?;
        Ok(())
    }
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting DingTalk channel");
        // Note: DingTalk uses webhooks/callbacks, actual event handling is via handle_callback_message
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping DingTalk channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        self.send_text(&msg.content).await
    }
}

// DingTalk API types

/// DingTalk callback message (for 2.0 robots)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DingTalkCallbackMessage {
    pub msgtype: String,
    pub text: DingTalkTextContent,
    pub msgid: String,
    pub createat: i64,
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "conversationType")]
    pub conversation_type: String,
    #[serde(rename = "conversationTitle")]
    pub conversation_title: Option<String>,
    #[serde(rename = "senderId")]
    pub sender_id: String,
    #[serde(rename = "senderNick")]
    pub sender_nick: String,
    #[serde(rename = "senderCorpId")]
    pub sender_corp_id: Option<String>,
    #[serde(rename = "senderStaffId")]
    pub sender_staff_id: Option<String>,
    #[serde(rename = "chatbotUserId")]
    pub chatbot_user_id: String,
    #[serde(rename = "atUsers")]
    pub at_users: Option<Vec<DingTalkAtUser>>,
}

/// DingTalk text content
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DingTalkTextContent {
    pub content: String,
}

/// DingTalk at user info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DingTalkAtUser {
    #[serde(rename = "dingtalkId")]
    pub dingtalk_id: String,
}

/// DingTalk webhook response
#[derive(Debug, Deserialize)]
pub struct DingTalkWebhookResponse {
    pub errcode: i32,
    pub errmsg: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> InboundSender {
        let (tx, _rx) = mpsc::channel(100);
        InboundSender::new(tx)
    }

    #[test]
    fn test_dingtalk_config_creation() {
        let config = DingTalkConfig {
            webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test123".to_string(),
            secret: Some("SEC123".to_string()),
            access_token: None,
            allow_from: vec![],
        };

        assert_eq!(
            config.webhook_url,
            "https://oapi.dingtalk.com/robot/send?access_token=test123"
        );
    }

    #[test]
    fn test_dingtalk_channel_creation() {
        let config = DingTalkConfig {
            webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test".to_string(),
            secret: None,
            access_token: None,
            allow_from: vec![],
        };

        let channel = DingTalkChannel::new(config, create_test_sender());

        assert_eq!(channel.name(), "dingtalk");
    }

    #[test]
    fn test_signed_webhook_url_without_secret() {
        let config = DingTalkConfig {
            webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test".to_string(),
            secret: None,
            access_token: None,
            allow_from: vec![],
        };

        let channel = DingTalkChannel::new(config, create_test_sender());

        let url = channel.get_signed_webhook_url();
        assert_eq!(
            url,
            "https://oapi.dingtalk.com/robot/send?access_token=test"
        );
    }

    #[test]
    fn test_parse_dingtalk_callback_message() {
        let json = r#"{
            "msgtype": "text",
            "text": {
                "content": "Hello from DingTalk!"
            },
            "msgid": "msg123",
            "createat": 1234567890000,
            "conversationId": "cid123",
            "conversationType": "1",
            "conversationTitle": "Test Chat",
            "senderId": "user123",
            "senderNick": "Test User",
            "chatbotUserId": "bot123"
        }"#;

        let message: DingTalkCallbackMessage = serde_json::from_str(json).unwrap();
        assert_eq!(message.msgtype, "text");
        assert_eq!(message.text.content, "Hello from DingTalk!");
        assert_eq!(message.sender_id, "user123");
    }

    #[test]
    fn test_dingtalk_text_message_serialization() {
        #[derive(Serialize)]
        struct TextMessage {
            msgtype: String,
            text: TextContent,
        }

        #[derive(Serialize)]
        struct TextContent {
            content: String,
        }

        let message = TextMessage {
            msgtype: "text".to_string(),
            text: TextContent {
                content: "Test message".to_string(),
            },
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"msgtype\":\"text\""));
        assert!(json.contains("\"content\":\"Test message\""));
    }
}
