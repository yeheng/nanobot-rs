//! Channel integrations
//!
//! This module provides channel abstractions for message routing:
//! - `base`: Core Channel trait for inbound channels
//! - `middleware`: Rate limiting, auth, and logging utilities
//! - `outbound`: Registry-based outbound message routing (preferred)
//!
//! # Outbound Routing
//!
//! Use `OutboundSenderRegistry` for extensible outbound message routing.
//! The legacy `send_outbound` function is deprecated.
//!
//! ```ignore
//! let registry = OutboundSenderRegistry::from_config(&config.channels);
//! registry.register_custom("sms".to_string(), Arc::new(MySmsSender::new()));
//! registry.send(msg).await?;
//! ```

pub mod base;
pub mod middleware;
pub mod outbound;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "dingtalk")]
pub mod dingtalk;

#[cfg(feature = "feishu")]
pub mod feishu;

#[cfg(feature = "wecom")]
pub mod wecom;

#[cfg(feature = "webhook")]
pub mod websocket;

pub use base::Channel;
pub use middleware::{
    log_inbound, ChannelError, InboundSender, SimpleAuthChecker, SimpleRateLimiter,
};
pub use outbound::{OutboundSender, OutboundSenderRegistry};

use crate::bus::events::OutboundMessage;
use crate::config::ChannelsConfig;
use crate::error::ChannelError as CoreChannelError;

/// Send an outbound message using the appropriate channel based on message.channel.
///
/// **Deprecated**: Use `OutboundSenderRegistry` instead for better extensibility.
///
/// This function is kept for backward compatibility. New code should use:
/// ```ignore
/// let registry = OutboundSenderRegistry::from_config(config);
/// registry.send(msg).await?;
/// ```
///
/// Routes to the appropriate channel's stateless send function.
#[allow(unused_variables)]
pub async fn send_outbound(
    config: &ChannelsConfig,
    msg: OutboundMessage,
) -> Result<(), CoreChannelError> {
    match msg.channel {
        #[cfg(feature = "telegram")]
        crate::bus::ChannelType::Telegram => {
            if let Some(ref tel) = config.telegram {
                telegram::send_text_stateless(&tel.token, &msg.chat_id, &msg.content)
                    .await
                    .map_err(|e| CoreChannelError::SendError(e.to_string()))
            } else {
                Err(CoreChannelError::NotConfigured("telegram".to_string()))
            }
        }

        #[cfg(feature = "discord")]
        crate::bus::ChannelType::Discord => {
            if let Some(ref discord) = config.discord {
                discord::send_message_stateless(&discord.token, &msg.chat_id, &msg.content)
                    .await
                    .map_err(|e| CoreChannelError::SendError(e.to_string()))
            } else {
                Err(CoreChannelError::NotConfigured("discord".to_string()))
            }
        }

        #[cfg(feature = "slack")]
        crate::bus::ChannelType::Slack => {
            if let Some(ref slack) = config.slack {
                slack::send_message_stateless(&slack.bot_token, &msg.chat_id, &msg.content, None)
                    .await
                    .map_err(|e| CoreChannelError::SendError(e.to_string()))
            } else {
                Err(CoreChannelError::NotConfigured("slack".to_string()))
            }
        }

        #[cfg(feature = "email")]
        crate::bus::ChannelType::Email => {
            if let Some(ref email) = config.email {
                if let (Some(ref host), Some(ref user), Some(ref pass), Some(ref from)) = (
                    &email.smtp_host,
                    &email.smtp_username,
                    &email.smtp_password,
                    &email.from_address,
                ) {
                    email::send_email_stateless(
                        host,
                        email.smtp_port,
                        user,
                        pass,
                        from,
                        msg.chat_id.trim_start_matches("email:"),
                        "Re: Your message",
                        &msg.content,
                    )
                    .await
                    .map_err(|e| CoreChannelError::SendError(e.to_string()))
                } else {
                    Err(CoreChannelError::NotConfigured(
                        "email SMTP not fully configured".to_string(),
                    ))
                }
            } else {
                Err(CoreChannelError::NotConfigured("email".to_string()))
            }
        }

        #[cfg(feature = "feishu")]
        crate::bus::ChannelType::Feishu => {
            if let Some(ref feishu) = config.feishu {
                feishu::send_text_stateless(
                    &feishu.app_id,
                    &feishu.app_secret,
                    &msg.chat_id,
                    &msg.content,
                )
                .await
                .map_err(|e| CoreChannelError::SendError(e.to_string()))
            } else {
                Err(CoreChannelError::NotConfigured("feishu".to_string()))
            }
        }

        crate::bus::ChannelType::WebSocket => {
            // WebSocket outbound is handled by the WebSocket connection itself.
            // This is a placeholder or can be used for logging.
            tracing::debug!("WebSocket outbound message: {}", msg.content);
            Ok(())
        }

        // Unsupported channels
        _ => {
            tracing::warn!(
                "No outbound handler for channel {:?}, dropping message to {}",
                msg.channel,
                msg.chat_id
            );
            Ok(())
        }
    }
}
