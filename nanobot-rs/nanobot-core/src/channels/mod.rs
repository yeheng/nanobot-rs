//! Channel integrations

pub mod base;
pub mod manager;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "feishu")]
pub mod feishu;

pub use base::Channel;
pub use manager::ChannelManager;
