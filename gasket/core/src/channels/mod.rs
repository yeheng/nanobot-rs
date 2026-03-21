//! Channel integrations — re-exports from gasket-channels crate
//!
//! This module re-exports the channel abstractions and implementations
//! from the `gasket-channels` crate, maintaining backward compatibility
//! for all `crate::channels::*` imports within gasket-core.

pub mod base {
    pub use gasket_channels::base::*;
}
pub mod middleware {
    pub use gasket_channels::middleware::*;
}
pub mod outbound {
    pub use gasket_channels::outbound::*;
}

#[cfg(feature = "telegram")]
pub mod telegram {
    pub use gasket_channels::telegram::*;
}
#[cfg(feature = "discord")]
pub mod discord {
    pub use gasket_channels::discord::*;
}
#[cfg(feature = "slack")]
pub mod slack {
    pub use gasket_channels::slack::*;
}
#[cfg(feature = "email")]
pub mod email {
    pub use gasket_channels::email::*;
}
#[cfg(feature = "dingtalk")]
pub mod dingtalk {
    pub use gasket_channels::dingtalk::*;
}
#[cfg(feature = "feishu")]
pub mod feishu {
    pub use gasket_channels::feishu::*;
}
#[cfg(feature = "wecom")]
pub mod wecom {
    pub use gasket_channels::wecom::*;
}
#[cfg(feature = "webhook")]
pub mod websocket {
    pub use gasket_channels::websocket::*;
}

// Convenience re-exports
pub use gasket_channels::{
    log_inbound, Channel, ChannelError, InboundSender, OutboundSender, OutboundSenderRegistry,
    SimpleAuthChecker, SimpleRateLimiter,
};

use crate::bus::events::OutboundMessage;
use crate::config::ChannelsConfig;
use crate::error::ChannelError as CoreChannelError;

/// Send an outbound message using the appropriate channel based on message.channel.
///
/// **Deprecated**: Use `OutboundSenderRegistry` instead for better extensibility.
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
