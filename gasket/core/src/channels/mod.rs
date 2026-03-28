//! Channel system
//!
//! This module re-exports types from the `gasket-channels` crate.

pub use gasket_channels::{
    base, log_inbound, middleware, outbound, Channel, ChannelConfigError, ChannelType,
    ChannelsConfig, DingTalkConfig, DiscordConfig, EmailConfig, FeishuConfig, InboundMessage,
    InboundSender, MediaAttachment, OutboundMessage, OutboundSender, OutboundSenderRegistry,
    SessionKey, SessionKeyParseError, SimpleAuthChecker, SimpleRateLimiter, SlackConfig,
    TelegramConfig, WebSocketMessage,
};

#[cfg(any(
    feature = "webhook",
    feature = "dingtalk",
    feature = "feishu",
    feature = "wecom"
))]
pub use gasket_channels::webhook;

// Re-export platform modules (feature-gated)
#[cfg(feature = "dingtalk")]
pub use gasket_channels::dingtalk;
#[cfg(feature = "discord")]
pub use gasket_channels::discord;
#[cfg(feature = "email")]
pub use gasket_channels::email;
#[cfg(feature = "feishu")]
pub use gasket_channels::feishu;
#[cfg(feature = "slack")]
pub use gasket_channels::slack;
#[cfg(feature = "telegram")]
pub use gasket_channels::telegram;
#[cfg(feature = "webhook")]
pub use gasket_channels::websocket;
#[cfg(feature = "wecom")]
pub use gasket_channels::wecom;
