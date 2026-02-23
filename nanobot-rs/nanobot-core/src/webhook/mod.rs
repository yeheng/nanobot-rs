//! Webhook HTTP server for receiving callbacks from messaging platforms
//!
//! This module provides a unified HTTP server that handles webhook callbacks
//! from various messaging platforms like WeCom, Feishu, DingTalk, etc.
//!
//! # Example
//!
//! ```ignore
//! use nanobot_core::webhook::{WebhookServer, WebhookConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = WebhookConfig {
//!         port: 3000,
//!         host: "0.0.0.0".to_string(),
//!     };
//!
//!     let server = WebhookServer::new(config);
//!     server.start().await?;
//! }
//! ```

mod handlers;
mod server;
mod types;

#[cfg(feature = "dingtalk")]
mod dingtalk;
#[cfg(feature = "feishu")]
mod feishu;
#[cfg(feature = "wecom")]
mod wecom;

pub use server::{WebhookConfig, WebhookServer};
pub use types::{WebhookError, WebhookResult};

#[cfg(feature = "dingtalk")]
pub use dingtalk::DingTalkWebhookHandler;
#[cfg(feature = "feishu")]
pub use feishu::FeishuWebhookHandler;
#[cfg(feature = "wecom")]
pub use wecom::WeComWebhookHandler;
