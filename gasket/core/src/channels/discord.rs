//! Discord channel implementation using serenity

use async_trait::async_trait;
use serenity::all::{GatewayIntents, Message as DiscordMessage};
use serenity::prelude::*;
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, instrument};

use super::base::Channel;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;

/// Discord channel configuration
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub allow_from: Vec<String>,
}

/// Discord channel.
///
/// Sends incoming messages directly to the message bus via `Sender<InboundMessage>`.
pub struct DiscordChannel {
    config: DiscordConfig,
    inbound_sender: Sender<InboundMessage>,
}

impl DiscordChannel {
    /// Create a new Discord channel with an inbound message sender.
    pub fn new(config: DiscordConfig, inbound_sender: Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_sender,
        }
    }

    /// Start the Discord bot
    #[instrument(name = "channel.discord.start", skip_all)]
    pub async fn start_bot(&self) -> anyhow::Result<()> {
        info!("Starting Discord bot");

        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let token = self.config.token.clone();
        let inbound_sender = self.inbound_sender.clone();
        let allow_from = self.config.allow_from.clone();

        let handler = DiscordHandler {
            inbound_sender,
            allow_from,
        };

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
}

/// Stateless send: send a message to Discord without needing a `DiscordChannel` instance.
pub async fn send_message_stateless(
    token: &str,
    channel_id: &str,
    content: &str,
) -> anyhow::Result<()> {
    use serenity::http::Http;
    use serenity::model::id::ChannelId;

    let http = Http::new(token);
    let channel_id: u64 = channel_id.parse()?;
    let channel = ChannelId::new(channel_id);

    channel.say(&http, content).await?;
    Ok(())
}

/// Discord event handler
struct DiscordHandler {
    inbound_sender: Sender<InboundMessage>,
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
