//! Slack adapter using Socket Mode

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage};
use crate::middleware::InboundSender;

#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    pub group_policy: Option<String>,
    pub allow_from: Vec<String>,
}

impl From<&crate::config::SlackConfig> for SlackConfig {
    fn from(cfg: &crate::config::SlackConfig) -> Self {
        Self {
            bot_token: cfg.bot_token.clone(),
            app_token: cfg.app_token.clone(),
            group_policy: cfg.group_policy.clone(),
            allow_from: cfg.allow_from.clone(),
        }
    }
}

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

/// Slack IM adapter.
#[derive(Clone)]
pub struct SlackAdapter {
    config: SlackConfig,
}

impl SlackAdapter {
    pub fn from_config(cfg: &crate::config::SlackConfig, _inbound: InboundSender) -> Self {
        Self { config: cfg.into() }
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
        inbound_sender: &InboundSender,
        group_policy: &Option<String>,
    ) where
        W: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    {
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let event_type = event["type"].as_str().unwrap_or("");

        if event_type == "events_api" {
            if let Some(envelope_id) = event.get("envelope_id").and_then(|v| v.as_str()) {
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
                            if channel.starts_with('C') {
                                match group_policy.as_deref() {
                                    Some("open") => {}
                                    _ => {
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
                                override_phase: None,
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
}

#[async_trait]
impl ImAdapter for SlackAdapter {
    fn name(&self) -> &str {
        "slack"
    }

    #[instrument(name = "adapter.slack.start", skip_all)]
    async fn start(&self, inbound_sender: InboundSender) -> anyhow::Result<()> {
        info!("Starting Slack adapter");

        let ws_url = self.get_socket_url().await?;
        info!("Connecting to Slack WebSocket: {}", ws_url);

        use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Slack WebSocket connected");

        let group_policy = self.config.group_policy.clone();

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

    async fn send(&self, msg: &crate::events::OutboundMessage) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = "https://slack.com/api/chat.postMessage";

        let mut body = serde_json::json!({
            "channel": msg.chat_id(),
            "text": msg.content(),
        });

        if let Some(ref meta) = msg.metadata {
            if let Some(ts) = meta.get("thread_ts").and_then(|v| v.as_str()) {
                body["thread_ts"] = serde_json::json!(ts);
            }
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
