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

mod server;
mod handlers;
mod types;

#[cfg(feature = "wecom")]
mod wecom;
#[cfg(feature = "feishu")]
mod feishu;
#[cfg(feature = "dingtalk")]
mod dingtalk;

pub use server::{WebhookServer, WebhookConfig};
pub use types::{WebhookError, WebhookResult};

#[cfg(feature = "wecom")]
pub use wecom::WeComWebhookHandler;
#[cfg(feature = "feishu")]
pub use feishu::FeishuWebhookHandler;
#[cfg(feature = "dingtalk")]
pub use dingtalk::DingTalkWebhookHandler;
