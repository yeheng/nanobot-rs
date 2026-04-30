//! WeChat (个人微信) adapter using wechatbot iLink SDK.
//!
//! Bridges the wechatbot crate's long-poll API to gasket's `ImAdapter` trait.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, instrument};
use wechatbot::{BotOptions, WeChatBot};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage};
use crate::middleware::InboundSender;

/// Runtime configuration for the WeChat adapter.
#[derive(Debug, Clone)]
pub struct WechatConfig {
    pub base_url: Option<String>,
    pub cred_path: Option<String>,
    pub allow_from: Vec<String>,
}

impl From<&crate::config::WechatConfig> for WechatConfig {
    fn from(cfg: &crate::config::WechatConfig) -> Self {
        Self {
            base_url: cfg.base_url.clone(),
            cred_path: cfg.cred_path.clone(),
            allow_from: cfg.allow_from.clone(),
        }
    }
}

/// WeChat IM adapter.
///
/// Wraps `WeChatBot` in an `Arc` so the same instance can be shared
/// between the inbound long-poll loop and outbound send calls.
#[derive(Clone)]
pub struct WechatAdapter {
    config: WechatConfig,
    bot: Arc<WeChatBot>,
}

impl WechatAdapter {
    pub fn from_config(cfg: &crate::config::WechatConfig, _inbound: InboundSender) -> Self {
        let opts = BotOptions {
            base_url: cfg.base_url.clone(),
            cred_path: cfg.cred_path.clone(),
            on_qr_url: Some(Box::new(|url| {
                tracing::info!("WeChat login QR code: {}", url);
            })),
            on_error: Some(Box::new(|err| {
                tracing::error!("WeChat bot error: {}", err);
            })),
        };
        let bot = Arc::new(WeChatBot::new(opts));
        Self {
            config: cfg.into(),
            bot,
        }
    }
}

#[async_trait]
impl ImAdapter for WechatAdapter {
    fn name(&self) -> &str {
        "wechat"
    }

    #[instrument(name = "adapter.wechat.start", skip_all)]
    async fn start(&self, inbound_sender: InboundSender) -> anyhow::Result<()> {
        info!("Starting WeChat adapter");

        // Attempt login (reuses stored credentials if available).
        if let Err(e) = self.bot.login(false).await {
            tracing::error!("WeChat login failed: {}", e);
            return Err(e.into());
        }

        let allow_from = self.config.allow_from.clone();
        let inbound_sender = inbound_sender.clone();

        // Register message handler.
        // The callback is synchronous, so we spawn an async task for each
        // inbound message to avoid blocking the poll loop.
        self.bot
            .on_message(Box::new(move |msg| {
                let user_id = msg.user_id.clone();

                if !allow_from.is_empty() && !allow_from.contains(&user_id) {
                    debug!(
                        "Ignoring message from unauthorized WeChat user: {}",
                        user_id
                    );
                    return;
                }

                let text = msg.text.clone();
                let inbound_sender = inbound_sender.clone();

                tokio::spawn(async move {
                    let inbound = InboundMessage {
                        channel: ChannelType::Wechat,
                        sender_id: user_id.clone(),
                        chat_id: user_id,
                        content: text,
                        media: None,
                        metadata: None,
                        timestamp: chrono::Utc::now(),
                        trace_id: None,
                    override_phase: None,
                    };

                    if let Err(e) = inbound_sender.send(inbound).await {
                        debug!("Failed to send inbound WeChat message: {}", e);
                    }
                });
            }))
            .await;

        // Long-poll loop — blocks until the bot is stopped or the task is aborted.
        if let Err(e) = self.bot.run().await {
            tracing::error!("WeChat run loop exited with error: {}", e);
            return Err(e.into());
        }

        Ok(())
    }

    async fn send(&self, msg: &crate::events::OutboundMessage) -> anyhow::Result<()> {
        if msg.is_broadcast() {
            // Broadcasting is not supported for WeChat personal accounts
            // because we do not maintain a contact list.
            tracing::warn!("Broadcast not supported for WeChat channel, skipping");
            return Ok(());
        }

        let chat_id = msg.chat_id();
        let content = msg.content();

        if let Err(e) = self.bot.send(chat_id, content).await {
            // NoContext means we haven't received a message from this user yet.
            if e.to_string().contains("NoContext") {
                tracing::warn!(
                    "Cannot send WeChat message to {}: no context token yet (user must send a message first)",
                    chat_id
                );
            } else {
                tracing::error!("Failed to send WeChat message: {}", e);
            }
            return Err(e.into());
        }

        Ok(())
    }
}
