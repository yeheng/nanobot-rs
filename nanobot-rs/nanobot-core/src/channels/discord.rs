//! Discord channel implementation using serenity

use async_trait::async_trait;
use serenity::all::{GatewayIntents, Message as DiscordMessage};
use serenity::prelude::*;
use tracing::{debug, info};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::MessageBus;

/// Discord channel configuration
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub allow_from: Vec<String>,
}

/// Discord channel
pub struct DiscordChannel {
    config: DiscordConfig,
    bus: MessageBus,
}

impl DiscordChannel {
    /// Create a new Discord channel
    pub fn new(config: DiscordConfig, bus: MessageBus) -> Self {
        Self { config, bus }
    }

    /// Start the Discord bot
    pub async fn start(&self) -> anyhow::Result<()> {
        info!("Starting Discord bot");

        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let token = self.config.token.clone();
        let bus = self.bus.clone();
        let allow_from = self.config.allow_from.clone();

        let handler = DiscordHandler { bus, allow_from };

        let mut client = Client::builder(&token, intents)
            .event_handler(handler)
            .await?;

        client.start().await?;

        Ok(())
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping Discord channel");
        Ok(())
    }

    async fn send(&self, _msg: OutboundMessage) -> anyhow::Result<()> {
        // Note: Sending requires the client instance, handled differently
        Ok(())
    }
}

/// Discord event handler
struct DiscordHandler {
    bus: MessageBus,
    allow_from: Vec<String>,
}

#[serenity::async_trait]
impl EventHandler for DiscordHandler {
    async fn message(&self, _ctx: Context, msg: DiscordMessage) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        let user_id = msg.author.id.to_string();

        // Check allowlist
        if !self.allow_from.is_empty() && !self.allow_from.contains(&user_id) {
            debug!("Ignoring message from unauthorized user: {}", user_id);
            return;
        }

        debug!("Received message from {}: {}", user_id, msg.content);

        let inbound = InboundMessage {
            channel: "discord".to_string(),
            sender_id: user_id,
            chat_id: msg.channel_id.to_string(),
            content: msg.content.clone(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
        };

        self.bus.publish_inbound(inbound).await;
    }

    async fn ready(&self, _ctx: Context, ready: serenity::model::gateway::Ready) {
        info!("Discord bot ready: {}", ready.user.name);
    }
}
