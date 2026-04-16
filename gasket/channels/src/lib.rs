//! Messaging channel abstractions and implementations for gasket.
//!
//! This crate provides:
//! - Core channel types (`events`, `config`, `adapter`, `middleware`, `provider`)
//! - Feature-gated IM adapter implementations (Telegram, Discord, Slack, etc.)
//! - Platform-specific webhook handlers (DingTalk, Feishu, WeCom)
//!
//! # Platform Modules
//!
//! Each platform module contains both adapter and webhook implementations:
//!
//! - [`dingtalk`] - DingTalk (钉钉) adapter and webhook
//! - [`feishu`] - Feishu (飞书) adapter and webhook
//! - [`wecom`] - WeCom (企业微信) adapter, webhook, and crypto
//!
//! # WebSocket & CLI
//!
//! - [`websocket`] - WebSocket server and `WebSocketAdapter`/`CliAdapter`

// Core types (always compiled)
pub mod adapter;
pub mod config;
pub mod error;
pub mod events;
pub mod middleware;
pub mod provider;

// Webhook HTTP server infrastructure
// Enabled when any platform that needs webhooks is enabled, or when webhook feature is explicitly enabled
#[cfg(any(
    feature = "webhook",
    feature = "dingtalk",
    feature = "feishu",
    feature = "wecom"
))]
pub mod webhook;

// Platform adapter implementations (feature-gated)
#[cfg(feature = "dingtalk")]
pub mod dingtalk;
#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "feishu")]
pub mod feishu;
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "telegram")]
pub mod telegram;
#[cfg(feature = "webhook")]
pub mod websocket;
#[cfg(feature = "wecom")]
pub mod wecom;

// Convenience re-exports
pub use adapter::ImAdapter;
pub use config::{
    ChannelsConfig, DingTalkConfig, DiscordConfig, FeishuConfig, SlackConfig, TelegramConfig,
    WeComConfig,
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
