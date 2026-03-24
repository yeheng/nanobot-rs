//! Messaging channel abstractions and implementations for gasket.
//!
//! This crate provides:
//! - Core channel types (`events`, `config`, `base`, `middleware`, `outbound`)
//! - Feature-gated channel implementations (Telegram, Discord, Slack, etc.)
//! - Platform-specific webhook handlers (DingTalk, Feishu, WeCom)
//!
//! # Platform Modules
//!
//! Each platform module contains both channel and webhook implementations:
//!
//! - [`dingtalk`] - DingTalk (钉钉) channel and webhook
//! - [`feishu`] - Feishu (飞书) channel and webhook
//! - [`wecom`] - WeCom (企业微信) channel, webhook, and crypto
//!
//! # Webhook Server
//!
//! The [`webhook`] module provides a generic HTTP server that can be combined
//! with platform-specific routes.

// Core types (always compiled)
pub mod base;
pub mod config;
pub mod error;
pub mod events;
pub mod middleware;
pub mod outbound;

// Webhook HTTP server infrastructure
// Enabled when any platform that needs webhooks is enabled, or when webhook feature is explicitly enabled
#[cfg(any(
    feature = "webhook",
    feature = "dingtalk",
    feature = "feishu",
    feature = "wecom"
))]
pub mod webhook;

// Platform channel implementations (feature-gated)
#[cfg(feature = "dingtalk")]
pub mod dingtalk;
#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "email")]
pub mod email;
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
pub use base::Channel;
pub use config::{
    ChannelsConfig, DingTalkConfig, DiscordConfig, EmailConfig, FeishuConfig, SlackConfig,
    TelegramConfig,
};
pub use error::ChannelConfigError;
pub use events::{
    ChannelType, InboundMessage, MediaAttachment, OutboundMessage, SessionKey,
    SessionKeyParseError, WebSocketMessage,
};
pub use middleware::{
    log_inbound, ChannelError, InboundSender, SimpleAuthChecker, SimpleRateLimiter,
};
pub use outbound::{OutboundSender, OutboundSenderRegistry};
