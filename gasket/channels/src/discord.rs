//! Discord adapter using serenity

use async_trait::async_trait;
use serenity::all::{GatewayIntents, Message as DiscordMessage};
use serenity::builder::CreateMessage;
use serenity::http::Http;
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage};
use crate::middleware::InboundSender;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub allow_from: Vec<String>,
    pub proxy_url: Option<String>,
}

impl From<&crate::config::DiscordConfig> for DiscordConfig {
    fn from(cfg: &crate::config::DiscordConfig) -> Self {
        Self {
            token: cfg.token.clone(),
            allow_from: cfg.allow_from.clone(),
            proxy_url: cfg.proxy_url.clone(),
        }
    }
}

/// Discord IM adapter.
#[derive(Clone)]
pub struct DiscordAdapter {
    config: DiscordConfig,
    http: Arc<Http>,
}

impl DiscordAdapter {
    pub fn from_config(cfg: &crate::config::DiscordConfig, _inbound: InboundSender) -> Self {
        let config: DiscordConfig = cfg.into();
        let http = build_http(&config);
        Self { config, http }
    }
}

fn build_http(config: &DiscordConfig) -> Arc<Http> {
    let mut builder = serenity::http::HttpBuilder::new(&config.token);
    if let Some(ref proxy) = config.proxy_url {
        builder = builder.proxy(proxy);
        info!("Discord REST API proxy configured: {}", proxy);
    }
    Arc::new(builder.build())
}

#[async_trait]
impl ImAdapter for DiscordAdapter {
    fn name(&self) -> &str {
        "discord"
    }

    #[instrument(name = "adapter.discord.start", skip_all)]
    async fn start(&self, inbound_sender: InboundSender) -> anyhow::Result<()> {
        info!("Starting Discord adapter");

        if self.config.proxy_url.is_some() {
            warn!(
                "Discord REST API proxy is configured, but Gateway WebSocket connections \
                 require a system-level transparent proxy (e.g., TUN mode). \
                 If you see 'TimedOut' errors, ensure your system proxy is active."
            );
        }

        let intents = GatewayIntents::GUILDS
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let handler = DiscordHandler {
            inbound_sender,
            allow_from: self.config.allow_from.clone(),
        };

        let mut client = Client::builder(&self.config.token, intents)
            .event_handler(handler)
            .await?;

        client.start().await?;
        Ok(())
    }

    async fn send(&self, msg: &crate::events::OutboundMessage) -> anyhow::Result<()> {
        let channel_id = ChannelId::new(msg.chat_id().parse()?);

        let mut builder = CreateMessage::new().content(msg.content());

        // Reply to the original message if message_id is present in metadata.
        // This creates a Discord reply thread / message reference.
        if let Some(ref meta) = msg.metadata {
            if let Some(message_id) = meta.get("message_id").and_then(|v| v.as_str()) {
                let reply_id = message_id.parse()?;
                builder = builder.reference_message((channel_id, reply_id));
            }
        }

        channel_id.send_message(&self.http, builder).await?;
        Ok(())
    }
}

struct DiscordHandler {
    inbound_sender: InboundSender,
    allow_from: Vec<String>,
}

#[serenity::async_trait]
impl EventHandler for DiscordHandler {
    async fn message(&self, _ctx: Context, msg: DiscordMessage) {
        if msg.author.bot {
            return;
        }

        let user_id = msg.author.id.to_string();

        if !self.allow_from.is_empty() && !self.allow_from.contains(&user_id) {
            debug!("Ignoring message from unauthorized user: {}", user_id);
            return;
        }

        debug!("Received message from {}: {}", user_id, msg.content);

        let inbound = InboundMessage {
            channel: ChannelType::Discord,
            sender_id: user_id,
            chat_id: msg.channel_id.to_string(),
            content: msg.content.clone(),
            media: None,
            metadata: Some(serde_json::json!({
                "message_id": msg.id.to_string(),
            })),
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };

        if let Err(e) = self.inbound_sender.send(inbound).await {
            debug!("Failed to send inbound message: {}", e);
        }
    }

    async fn ready(&self, _ctx: Context, ready: serenity::model::gateway::Ready) {
        info!("Discord bot ready: {} ({})", ready.user.name, ready.user.id);
    }

    async fn resume(&self, _ctx: Context, _event: serenity::model::event::ResumedEvent) {
        info!("Discord gateway connection resumed");
    }
}
