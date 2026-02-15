//! Slack channel implementation using Socket Mode

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::MessageBus;

/// Slack channel configuration
#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    pub group_policy: Option<String>,
    pub allow_from: Vec<String>,
}

/// Slack message event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct SlackMessage {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
}

/// Slack channel using Socket Mode
pub struct SlackChannel {
    config: SlackConfig,
    bus: MessageBus,
}

impl SlackChannel {
    /// Create a new Slack channel
    pub fn new(config: SlackConfig, bus: MessageBus) -> Self {
        Self { config, bus }
    }

    /// Start the Slack bot using WebSocket
    pub async fn start(self) -> anyhow::Result<()> {
        info!("Starting Slack bot");

        let ws_url = self.get_socket_url().await?;
        info!("Connecting to Slack WebSocket: {}", ws_url);

        // Use tokio-tungstenite for WebSocket connection
        use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Slack WebSocket connected");

        // Handle messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        self.handle_event(&event, &mut write).await;
                    }
                }
                Ok(WsMessage::Ping(data)) => {
                    write.send(WsMessage::Pong(data)).await?;
                }
                Ok(WsMessage::Close(_)) => {
                    info!("Slack WebSocket closed");
                    break;
                }
                Err(e) => {
                    tracing::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn get_socket_url(&self) -> anyhow::Result<String> {
        let client = reqwest::Client::new();
        let url = "https://slack.com/api/apps.connections.open";

        let response = client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.app_token))
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let body = response.text().await?;
        let json: serde_json::Value = serde_json::from_str(&body)?;

        if json["ok"].as_bool() != Some(true) {
            anyhow::bail!(
                "Failed to get WebSocket URL: {}",
                json["error"].as_str().unwrap_or("unknown")
            );
        }

        Ok(json["url"].as_str().unwrap_or_default().to_string())
    }

    async fn handle_event<W>(&self, event: &serde_json::Value, write: &mut W)
    where
        W: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    {
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let event_type = event["type"].as_str().unwrap_or("");

        if event_type == "events_api" {
            if let Some(envelope_id) = event.get("envelope_id").and_then(|v| v.as_str()) {
                // Acknowledge the event first
                let ack = serde_json::json!({
                    "envelope_id": envelope_id
                });
                let _ = write.send(WsMessage::Text(ack.to_string().into())).await;
                debug!("Acknowledged Slack event: {}", envelope_id);
            }

            if let Some(payload) = event.get("payload") {
                if let Some(event_data) = payload.get("event") {
                    let msg_type = event_data["type"].as_str().unwrap_or("");

                    if msg_type == "message" {
                        // Skip bot messages
                        if event_data.get("bot_id").is_some()
                            || event_data["subtype"].as_str() == Some("bot_message")
                        {
                            return;
                        }

                        if let (Some(text), Some(channel), Some(user)) = (
                            event_data["text"].as_str(),
                            event_data["channel"].as_str(),
                            event_data["user"].as_str(),
                        ) {
                            // Check group policy for channel messages
                            if channel.starts_with('C') {
                                match self.config.group_policy.as_deref() {
                                    Some("open") => {
                                        // Respond to all
                                    }
                                    _ => {
                                        // Default: mention only
                                        if !text.contains("<@") {
                                            debug!("Skipping non-mention message in channel");
                                            return;
                                        }
                                    }
                                }
                            }

                            debug!("Received Slack message from {}: {}", user, text);

                            let inbound = InboundMessage {
                                channel: "slack".to_string(),
                                sender_id: user.to_string(),
                                chat_id: channel.to_string(),
                                content: text.to_string(),
                                media: None,
                                metadata: Some(serde_json::json!({
                                    "thread_ts": event_data["thread_ts"],
                                    "ts": event_data["ts"]
                                })),
                                timestamp: chrono::Utc::now(),
                            };

                            self.bus.publish_inbound(inbound).await;
                        }
                    }
                }
            }
        }
    }

    /// Send a message to Slack
    pub async fn send_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = "https://slack.com/api/chat.postMessage";

        let mut body = serde_json::json!({
            "channel": channel,
            "text": text,
        });

        if let Some(ts) = thread_ts {
            body["thread_ts"] = serde_json::json!(ts);
        }

        let response = client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let result: serde_json::Value = response.json().await?;
        if result["ok"].as_bool() != Some(true) {
            anyhow::bail!(
                "Failed to send Slack message: {}",
                result["error"].as_str().unwrap_or("unknown")
            );
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping Slack channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        let thread_ts = msg
            .metadata
            .as_ref()
            .and_then(|m| m.get("thread_ts"))
            .and_then(|v| v.as_str());
        self.send_message(&msg.chat_id, &msg.content, thread_ts)
            .await
    }
}
