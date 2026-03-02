//! Channel integrations

pub mod base;
pub mod middleware;

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

pub use base::Channel;
pub use middleware::{
    log_inbound, ChannelError, InboundSender, SimpleAuthChecker, SimpleRateLimiter,
};

use crate::bus::events::OutboundMessage;
use crate::config::ChannelsConfig;

/// Send an outbound message using the appropriate channel based on message.channel.
///
/// This is the single entry point for all outbound message sending.
/// Routes to the appropriate channel's stateless send function.
pub async fn send_outbound(config: &ChannelsConfig, msg: OutboundMessage) -> anyhow::Result<()> {
    match msg.channel {
        #[cfg(feature = "telegram")]
        crate::bus::ChannelType::Telegram => {
            if let Some(ref tel) = config.telegram {
                telegram::send_text_stateless(&tel.token, &msg.chat_id, &msg.content).await
            } else {
                anyhow::bail!("Telegram not configured")
            }
        }

        #[cfg(feature = "discord")]
        crate::bus::ChannelType::Discord => {
            if let Some(ref discord) = config.discord {
                discord::send_message_stateless(&discord.token, &msg.chat_id, &msg.content).await
            } else {
                anyhow::bail!("Discord not configured")
            }
        }

        #[cfg(feature = "slack")]
        crate::bus::ChannelType::Slack => {
            if let Some(ref slack) = config.slack {
                slack::send_message_stateless(&slack.bot_token, &msg.chat_id, &msg.content, None)
                    .await
            } else {
                anyhow::bail!("Slack not configured")
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
                } else {
                    anyhow::bail!("Email SMTP not fully configured")
                }
            } else {
                anyhow::bail!("Email not configured")
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
            } else {
                anyhow::bail!("Feishu not configured")
            }
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
