//! Discord adapter using serenity

use async_trait::async_trait;
use serenity::all::{GatewayIntents, Message as DiscordMessage};
use serenity::prelude::*;
use tracing::{debug, info, instrument};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage};
use crate::middleware::InboundSender;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub allow_from: Vec<String>,
}

impl From<&crate::config::DiscordConfig> for DiscordConfig {
    fn from(cfg: &crate::config::DiscordConfig) -> Self {
        Self {
            token: cfg.token.clone(),
            allow_from: cfg.allow_from.clone(),
        }
    }
}

/// Discord IM adapter.
#[derive(Clone)]
pub struct DiscordAdapter {
    config: DiscordConfig,
}

impl DiscordAdapter {
    pub fn from_config(cfg: &crate::config::DiscordConfig, _inbound: InboundSender) -> Self {
        Self { config: cfg.into() }
    }
}

#[async_trait]
impl ImAdapter for DiscordAdapter {
    fn name(&self) -> &str {
        "discord"
    }

    #[instrument(name = "adapter.discord.start", skip_all)]
    async fn start(&self, inbound_sender: InboundSender) -> anyhow::Result<()> {
        info!("Starting Discord adapter");

        let intents = GatewayIntents::GUILD_MESSAGES
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
        use serenity::http::Http;
        use serenity::model::id::ChannelId;

        let http = Http::new(&self.config.token);
        let channel_id: u64 = msg.chat_id().parse()?;
        let channel = ChannelId::new(channel_id);
        channel.say(&http, msg.content()).await?;
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
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };

        if let Err(e) = self.inbound_sender.send(inbound).await {
            debug!("Failed to send inbound message: {}", e);
        }
    }

    async fn ready(&self, _ctx: Context, ready: serenity::model::gateway::Ready) {
        info!("Discord bot ready: {}", ready.user.name);
    }
}
