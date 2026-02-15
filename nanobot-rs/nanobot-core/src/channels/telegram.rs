//! Telegram channel implementation using teloxide

use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tracing::{debug, info};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::MessageBus;

/// Telegram channel configuration
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub token: String,
    pub allow_from: Vec<String>,
}

/// Telegram channel
pub struct TelegramChannel {
    config: TelegramConfig,
    bus: MessageBus,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(config: TelegramConfig, bus: MessageBus) -> Self {
        Self { config, bus }
    }

    /// Start the Telegram bot (blocking)
    pub async fn start(self) -> anyhow::Result<()> {
        info!("Starting Telegram bot");

        let bot = Bot::new(&self.config.token);
        let bus = self.bus.clone();
        let allow_from = self.config.allow_from.clone();

        // Use Dispatcher for proper handling
        let handler = Update::filter_message().branch(dptree::endpoint(move |msg: Message| {
            let bus = bus.clone();
            let allow_from = allow_from.clone();
            async move {
                if let Some(ref user) = msg.from {
                    let user_id = user.id.0;
                    let user_id_str = user_id.to_string();

                    // Check allowlist
                    if !allow_from.is_empty() && !allow_from.contains(&user_id_str) {
                        debug!("Ignoring message from unauthorized user: {}", user_id);
                        return Ok::<_, teloxide::RequestError>(());
                    }

                    if let Some(text) = msg.text() {
                        let chat_id = msg.chat.id.0;

                        debug!("Received message from {}: {}", user_id, text);

                        // Publish to bus
                        let inbound = InboundMessage {
                            channel: "telegram".to_string(),
                            sender_id: user_id_str,
                            chat_id: chat_id.to_string(),
                            content: text.to_string(),
                            media: None,
                            metadata: None,
                            timestamp: chrono::Utc::now(),
                        };

                        bus.publish_inbound(inbound).await;
                    }
                }
                Ok(())
            }
        }));

        Dispatcher::builder(bot, handler)
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;

        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping Telegram channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        let bot = Bot::new(&self.config.token);
        let chat_id: i64 = msg.chat_id.parse()?;
        bot.send_message(ChatId(chat_id), &msg.content).await?;
        Ok(())
    }
}
