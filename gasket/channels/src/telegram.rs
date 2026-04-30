//! Telegram adapter using teloxide

use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tracing::{debug, info, instrument};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage};
use crate::middleware::InboundSender;

/// Telegram adapter configuration (runtime).
///
/// Uses the deserialized config directly — no duplicate struct.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub token: String,
    pub allow_from: Vec<String>,
}

impl From<&crate::config::TelegramConfig> for TelegramConfig {
    fn from(cfg: &crate::config::TelegramConfig) -> Self {
        Self {
            token: cfg.token.clone(),
            allow_from: cfg.allow_from.clone(),
        }
    }
}

/// Telegram IM adapter.
#[derive(Clone)]
pub struct TelegramAdapter {
    config: TelegramConfig,
}

impl TelegramAdapter {
    pub fn from_config(cfg: &crate::config::TelegramConfig, _inbound: InboundSender) -> Self {
        Self { config: cfg.into() }
    }
}

#[async_trait]
impl ImAdapter for TelegramAdapter {
    fn name(&self) -> &str {
        "telegram"
    }

    #[instrument(name = "adapter.telegram.start", skip_all)]
    async fn start(&self, inbound_sender: InboundSender) -> anyhow::Result<()> {
        info!("Starting Telegram adapter");

        let bot = Bot::new(&self.config.token);
        let inbound_sender = inbound_sender.clone();
        let allow_from = self.config.allow_from.clone();

        let handler = Update::filter_message().branch(dptree::endpoint(move |msg: Message| {
            let inbound_sender = inbound_sender.clone();
            let allow_from = allow_from.clone();
            async move {
                if let Some(ref user) = msg.from {
                    let user_id = user.id.0;
                    let user_id_str = user_id.to_string();

                    if !allow_from.is_empty() && !allow_from.contains(&user_id_str) {
                        debug!("Ignoring message from unauthorized user: {}", user_id);
                        return Ok::<_, teloxide::RequestError>(());
                    }

                    if let Some(text) = msg.text() {
                        let chat_id = msg.chat.id.0;

                        debug!("Received message from {}: {}", user_id, text);

                        let inbound = InboundMessage {
                            channel: ChannelType::Telegram,
                            sender_id: user_id_str,
                            chat_id: chat_id.to_string(),
                            content: text.to_string(),
                            media: None,
                            metadata: None,
                            timestamp: chrono::Utc::now(),
                            trace_id: None,
                        override_phase: None,
                        };

                        if let Err(e) = inbound_sender.send(inbound).await {
                            debug!("Failed to send inbound message: {}", e);
                        }
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

    async fn send(&self, msg: &crate::events::OutboundMessage) -> anyhow::Result<()> {
        let bot = Bot::new(&self.config.token);
        let chat_id: i64 = msg.chat_id().parse()?;
        bot.send_message(ChatId(chat_id), msg.content()).await?;
        Ok(())
    }
}
