//! Channel integrations

pub mod base;
pub mod manager;
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
pub use manager::ChannelManager;
pub use middleware::{
    log_inbound, log_outbound, ChannelError, SimpleAuthChecker, SimpleRateLimiter,
};
