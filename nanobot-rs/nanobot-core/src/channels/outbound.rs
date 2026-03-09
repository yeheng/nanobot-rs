//! Outbound message routing with registry pattern.
//!
//! This module provides a trait-based registry for outbound message routing,
//! replacing the large match statement in `send_outbound` with a more extensible
//! architecture that supports custom channels.
//!
//! # Example
//!
//! ```ignore
//! use nanobot_core::channels::outbound::{OutboundSenderRegistry, OutboundSender};
//!
//! // Create registry from config
//! let registry = OutboundSenderRegistry::from_config(&config);
//!
//! // Register a custom sender
//! registry.register_custom("sms".to_string(), Box::new(MySmsSender::new()));
//!
//! // Send via registry - works for all channels including custom
//! registry.send(msg).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::bus::events::{ChannelType, OutboundMessage};
use crate::channels::middleware::ChannelError;
use crate::config::ChannelsConfig;

/// Trait for sending outbound messages to a specific channel.
///
/// Each channel (Telegram, Discord, etc.) implements this trait to handle
/// its own message delivery logic.
#[async_trait]
pub trait OutboundSender: Send + Sync {
    /// Send an outbound message.
    ///
    /// Returns `Ok(())` on success, or a `ChannelError` on failure.
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError>;

    /// Returns the name of this sender for logging/debugging.
    fn name(&self) -> &str;
}

/// Registry for outbound message senders.
///
/// Manages a collection of `OutboundSender` implementations and routes
/// messages to the appropriate sender based on the channel type.
pub struct OutboundSenderRegistry {
    /// Senders for built-in channels (keyed by ChannelType)
    senders: HashMap<ChannelType, Arc<dyn OutboundSender>>,

    /// Senders for custom channels (keyed by custom name)
    custom_senders: HashMap<String, Arc<dyn OutboundSender>>,
}

impl OutboundSenderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            senders: HashMap::new(),
            custom_senders: HashMap::new(),
        }
    }

    /// Create a registry populated from channel configuration.
    ///
    /// This automatically registers senders for all configured channels.
    pub fn from_config(config: &ChannelsConfig) -> Self {
        let mut registry = Self::new();

        #[cfg(feature = "telegram")]
        if let Some(ref tel_config) = config.telegram {
            if tel_config.enabled {
                registry.register(
                    ChannelType::Telegram,
                    Arc::new(TelegramSender::new(tel_config.token.clone())),
                );
            }
        }

        #[cfg(feature = "discord")]
        if let Some(ref discord_config) = config.discord {
            if discord_config.enabled {
                registry.register(
                    ChannelType::Discord,
                    Arc::new(DiscordSender::new(discord_config.token.clone())),
                );
            }
        }

        #[cfg(feature = "slack")]
        if let Some(ref slack_config) = config.slack {
            if slack_config.enabled {
                registry.register(
                    ChannelType::Slack,
                    Arc::new(SlackSender::new(slack_config.bot_token.clone())),
                );
            }
        }

        #[cfg(feature = "feishu")]
        if let Some(ref feishu_config) = config.feishu {
            if feishu_config.enabled {
                registry.register(
                    ChannelType::Feishu,
                    Arc::new(FeishuSender::new(
                        feishu_config.app_id.clone(),
                        feishu_config.app_secret.clone(),
                    )),
                );
            }
        }

        #[cfg(feature = "email")]
        if let Some(ref email_config) = config.email {
            if email_config.enabled && email_config.has_smtp_config() {
                registry.register(
                    ChannelType::Email,
                    Arc::new(EmailSender::from_config(email_config)),
                );
            }
        }

        #[cfg(feature = "dingtalk")]
        if let Some(ref dingtalk_config) = config.dingtalk {
            if dingtalk_config.enabled {
                registry.register(
                    ChannelType::Dingtalk,
                    Arc::new(DingTalkSender::new(
                        dingtalk_config.webhook_url.clone(),
                        dingtalk_config.secret.clone(),
                    )),
                );
            }
        }

        // WebSocket is handled specially - no HTTP sender needed
        registry.register(ChannelType::WebSocket, Arc::new(WebSocketSender));

        // CLI channel - no-op for outbound
        registry.register(ChannelType::Cli, Arc::new(CliSender));

        registry
    }

    /// Register a sender for a built-in channel type.
    pub fn register(&mut self, channel: ChannelType, sender: Arc<dyn OutboundSender>) {
        debug!("Registering outbound sender for channel: {}", channel);
        self.senders.insert(channel, sender);
    }

    /// Register a sender for a custom channel.
    ///
    /// This allows extending the system with new channels without modifying
    /// core code. The sender will be invoked for `ChannelType::Custom(name)`.
    pub fn register_custom(&mut self, name: String, sender: Arc<dyn OutboundSender>) {
        debug!("Registering custom outbound sender: {}", name);
        self.custom_senders.insert(name, sender);
    }

    /// Send an outbound message via the appropriate sender.
    ///
    /// Returns `Ok(())` if the message was sent successfully or if there
    /// is no registered sender (logs a warning in the latter case).
    pub async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        let sender = self.get_sender(&msg.channel);

        match sender {
            Some(sender) => {
                debug!(
                    "Routing outbound message to {} sender for chat {}",
                    sender.name(),
                    msg.chat_id
                );
                sender.send(msg).await
            }
            None => {
                warn!(
                    "No outbound handler for channel {:?}, dropping message to {}",
                    msg.channel, msg.chat_id
                );
                // Return Ok to avoid breaking the pipeline - unregistered channels
                // are treated as no-ops rather than errors
                Ok(())
            }
        }
    }

    /// Get the sender for a channel type.
    fn get_sender(&self, channel: &ChannelType) -> Option<Arc<dyn OutboundSender>> {
        match channel {
            ChannelType::Custom(name) => self.custom_senders.get(name).cloned(),
            _ => self.senders.get(channel).cloned(),
        }
    }

    /// Check if a sender is registered for the given channel.
    pub fn has_sender(&self, channel: &ChannelType) -> bool {
        match channel {
            ChannelType::Custom(name) => self.custom_senders.contains_key(name),
            _ => self.senders.contains_key(channel),
        }
    }
}

impl Default for OutboundSenderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Built-in Channel Senders ──────────────────────────────────────────────

/// Telegram outbound sender.
#[cfg(feature = "telegram")]
pub struct TelegramSender {
    token: String,
}

#[cfg(feature = "telegram")]
impl TelegramSender {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

#[cfg(feature = "telegram")]
#[async_trait]
impl OutboundSender for TelegramSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        super::telegram::send_text_stateless(&self.token, &msg.chat_id, &msg.content)
            .await
            .map_err(|e| ChannelError::DeliveryFailed {
                channel: "telegram".to_string(),
                message: e.to_string(),
            })
    }

    fn name(&self) -> &str {
        "telegram"
    }
}

/// Discord outbound sender.
#[cfg(feature = "discord")]
pub struct DiscordSender {
    token: String,
}

#[cfg(feature = "discord")]
impl DiscordSender {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

#[cfg(feature = "discord")]
#[async_trait]
impl OutboundSender for DiscordSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        super::discord::send_message_stateless(&self.token, &msg.chat_id, &msg.content)
            .await
            .map_err(|e| ChannelError::DeliveryFailed {
                channel: "discord".to_string(),
                message: e.to_string(),
            })
    }

    fn name(&self) -> &str {
        "discord"
    }
}

/// Slack outbound sender.
#[cfg(feature = "slack")]
pub struct SlackSender {
    bot_token: String,
}

#[cfg(feature = "slack")]
impl SlackSender {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }
}

#[cfg(feature = "slack")]
#[async_trait]
impl OutboundSender for SlackSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        super::slack::send_message_stateless(&self.bot_token, &msg.chat_id, &msg.content, None)
            .await
            .map_err(|e| ChannelError::DeliveryFailed {
                channel: "slack".to_string(),
                message: e.to_string(),
            })
    }

    fn name(&self) -> &str {
        "slack"
    }
}

/// Feishu outbound sender.
#[cfg(feature = "feishu")]
pub struct FeishuSender {
    app_id: String,
    app_secret: String,
}

#[cfg(feature = "feishu")]
impl FeishuSender {
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self { app_id, app_secret }
    }
}

#[cfg(feature = "feishu")]
#[async_trait]
impl OutboundSender for FeishuSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        super::feishu::send_text_stateless(
            &self.app_id,
            &self.app_secret,
            &msg.chat_id,
            &msg.content,
        )
        .await
        .map_err(|e| ChannelError::DeliveryFailed {
            channel: "feishu".to_string(),
            message: e.to_string(),
        })
    }

    fn name(&self) -> &str {
        "feishu"
    }
}

/// Email outbound sender.
#[cfg(feature = "email")]
pub struct EmailSender {
    smtp_host: String,
    smtp_port: u16,
    smtp_username: String,
    smtp_password: String,
    from_address: String,
}

#[cfg(feature = "email")]
impl EmailSender {
    pub fn from_config(config: &crate::config::EmailConfig) -> Self {
        Self {
            smtp_host: config.smtp_host.clone().unwrap_or_default(),
            smtp_port: config.smtp_port,
            smtp_username: config.smtp_username.clone().unwrap_or_default(),
            smtp_password: config.smtp_password.clone().unwrap_or_default(),
            from_address: config.from_address.clone().unwrap_or_default(),
        }
    }
}

#[cfg(feature = "email")]
#[async_trait]
impl OutboundSender for EmailSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        let to = msg.chat_id.trim_start_matches("email:");
        super::email::send_email_stateless(
            &self.smtp_host,
            self.smtp_port,
            &self.smtp_username,
            &self.smtp_password,
            &self.from_address,
            to,
            "Re: Your message",
            &msg.content,
        )
        .await
        .map_err(|e| ChannelError::DeliveryFailed {
            channel: "email".to_string(),
            message: e.to_string(),
        })
    }

    fn name(&self) -> &str {
        "email"
    }
}

/// DingTalk outbound sender.
#[cfg(feature = "dingtalk")]
pub struct DingTalkSender {
    webhook_url: String,
    secret: Option<String>,
}

#[cfg(feature = "dingtalk")]
impl DingTalkSender {
    pub fn new(webhook_url: String, secret: Option<String>) -> Self {
        Self {
            webhook_url,
            secret,
        }
    }
}

#[cfg(feature = "dingtalk")]
#[async_trait]
impl OutboundSender for DingTalkSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        super::dingtalk::send_message_stateless(
            &self.webhook_url,
            self.secret.as_deref(),
            &msg.content,
        )
        .await
        .map_err(|e| ChannelError::DeliveryFailed {
            channel: "dingtalk".to_string(),
            message: e.to_string(),
        })
    }

    fn name(&self) -> &str {
        "dingtalk"
    }
}

/// WebSocket sender (placeholder - actual sending handled by WebSocketManager).
pub struct WebSocketSender;

#[async_trait]
impl OutboundSender for WebSocketSender {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError> {
        // WebSocket outbound is handled by the WebSocket connection itself.
        // This is a placeholder for logging.
        debug!("WebSocket outbound message: {}", msg.content);
        Ok(())
    }

    fn name(&self) -> &str {
        "websocket"
    }
}

/// CLI sender (no-op for outbound messages).
pub struct CliSender;

#[async_trait]
impl OutboundSender for CliSender {
    async fn send(&self, _msg: OutboundMessage) -> Result<(), ChannelError> {
        // CLI doesn't have outbound - responses are printed directly
        Ok(())
    }

    fn name(&self) -> &str {
        "cli"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = OutboundSenderRegistry::new();
        assert!(!registry.has_sender(&ChannelType::Telegram));
        assert!(!registry.has_sender(&ChannelType::Custom("sms".to_string())));
    }

    #[test]
    fn test_register_and_check() {
        let mut registry = OutboundSenderRegistry::new();
        registry.register(ChannelType::Cli, Arc::new(CliSender));
        assert!(registry.has_sender(&ChannelType::Cli));
    }

    #[test]
    fn test_custom_channel_registration() {
        let mut registry = OutboundSenderRegistry::new();

        // Custom channel not registered initially
        let custom = ChannelType::Custom("sms".to_string());
        assert!(!registry.has_sender(&custom));

        // Register custom sender
        registry.register_custom("sms".to_string(), Arc::new(CliSender));

        // Now it should be found
        assert!(registry.has_sender(&custom));
    }

    #[tokio::test]
    async fn test_unregistered_channel_returns_ok() {
        let registry = OutboundSenderRegistry::new();
        let msg =
            OutboundMessage::new(ChannelType::Custom("unknown".to_string()), "chat1", "hello");

        // Should return Ok (not error) for unregistered channels
        let result = registry.send(msg).await;
        assert!(result.is_ok());
    }
}
