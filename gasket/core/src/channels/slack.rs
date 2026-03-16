//! Slack channel implementation using Socket Mode

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, instrument};

use super::base::Channel;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;

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

/// Slack channel using Socket Mode.
///
/// Sends incoming messages directly to the message bus via `Sender<InboundMessage>`.
pub struct SlackChannel {
    config: SlackConfig,
    inbound_sender: Sender<InboundMessage>,
}

impl SlackChannel {
    /// Create a new Slack channel with an inbound message sender.
    pub fn new(config: SlackConfig, inbound_sender: Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_sender,
        }
    }

    /// Start the Slack bot using WebSocket
    #[instrument(name = "channel.slack.start", skip_all)]
    pub async fn start_bot(&self) -> anyhow::Result<()> {
        info!("Starting Slack bot");

        let ws_url = self.get_socket_url().await?;
        info!("Connecting to Slack WebSocket: {}", ws_url);

        // Use tokio-tungstenite for WebSocket connection
        use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Slack WebSocket connected");

        let inbound_sender = self.inbound_sender.clone();
        let group_policy = self.config.group_policy.clone();

        // Handle messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        Self::handle_event(&event, &mut write, &inbound_sender, &group_policy)
                            .await;
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

    async fn handle_event<W>(
        event: &serde_json::Value,
        write: &mut W,
        inbound_sender: &Sender<InboundMessage>,
        group_policy: &Option<String>,
    ) where
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
                                match group_policy.as_deref() {
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
                                channel: ChannelType::Slack,
                                sender_id: user.to_string(),
                                chat_id: channel.to_string(),
                                content: text.to_string(),
                                media: None,
                                metadata: Some(serde_json::json!({
                                    "thread_ts": event_data["thread_ts"],
                                    "ts": event_data["ts"]
                                })),
                                timestamp: chrono::Utc::now(),
                                trace_id: None,
                            };

                            if let Err(e) = inbound_sender.send(inbound).await {
                                debug!("Failed to send inbound message: {}", e);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Send a message to Slack
    #[instrument(name = "channel.slack.send_message", skip_all, fields(channel = %channel))]
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
}

/// Stateless send: post a message to Slack without needing a `SlackChannel` instance.
pub async fn send_message_stateless(
    bot_token: &str,
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
        .header("Authorization", format!("Bearer {}", bot_token))
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
