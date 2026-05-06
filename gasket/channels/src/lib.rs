//! Messaging channel abstractions and implementations for gasket.
//!
//! This crate provides:
//! - Core channel types (`events`, `config`, `adapter`, `middleware`, `provider`)
//! - Feature-gated IM adapter implementations (Telegram, Discord, Slack, etc.)
//! - Platform-specific webhook handlers (Feishu)
//!
//! # Platform Modules
//!
//! Each platform module contains both adapter and webhook implementations:
//!
//! - [`feishu`] - Feishu (飞书) adapter and webhook

//!
//! # WebSocket & CLI
//!
//! - [`websocket`] - WebSocket server and `WebSocketAdapter`/`CliAdapter`

// Core types (always compiled)
pub mod adapter;
pub mod approval_router;
pub mod config;
pub mod error;
pub mod events;
pub mod middleware;
pub mod provider;

// Platform adapter implementations (feature-gated)
#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "feishu")]
pub mod feishu;
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "telegram")]
pub mod telegram;
#[cfg(feature = "websocket")]
pub mod websocket;
#[cfg(feature = "wechat")]
pub mod wechat;


// Convenience re-exports
pub use adapter::ImAdapter;
pub use approval_router::ApprovalRouter;
pub use config::{
    ChannelsConfig, DiscordConfig, FeishuConfig, SlackConfig, TelegramConfig,
    WebSocketConfig, WechatConfig,
};
pub use error::ChannelConfigError;
pub use events::{
    ChannelType, InboundMessage, MediaAttachment, OutboundMessage, SessionKey,
    SessionKeyParseError, WebSocketMessage,
};
pub use middleware::{
    log_inbound, ChannelError, InboundSender, SimpleAuthChecker, SimpleRateLimiter,
};
pub use provider::{ImProvider, ImProviders};

#[cfg(feature = "websocket")]
pub use websocket::WebSocketApprovalCallback;
