//! Webhook HTTP server for receiving callbacks from messaging platforms
//!
//! This module provides HTTP routes that handle webhook callbacks
//! from various messaging platforms like WeCom, Feishu, DingTalk, etc.
//!
//! # Example
//!
//! ```ignore
//! use gasket_core::webhook::{WebhookServer, WebhookConfig};
//! use gasket_core::webhook::wecom::{create_wecom_routes, WeComState};
//! use gasket_core::webhook::feishu::{create_feishu_routes, FeishuState};
//! use gasket_core::webhook::dingtalk::{create_dingtalk_routes, DingTalkState};
//! use gasket_core::channels::wecom::WeComConfig;
//! use gasket_core::channels::feishu::FeishuConfig;
//! use gasket_core::channels::dingtalk::DingTalkConfig;
//! use gasket_core::bus::events::InboundMessage;
//! use tokio::sync::mpsc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = WebhookConfig {
//!         port: 3000,
//!         host: "0.0.0.0".to_string(),
//!     };
//!
//!     // Create a channel for inbound messages
//!     let (tx, rx) = mpsc::channel::<InboundMessage>(100);
//!
//!     // Create platform-specific routes
//!     let wecom_state = WeComState::from_config(WeComConfig::default(), tx.clone());
//!     let wecom_router = create_wecom_routes(wecom_state, None);
//!
//!     let feishu_state = FeishuState::from_config(FeishuConfig::default(), tx.clone());
//!     let feishu_router = create_feishu_routes(feishu_state, None);
//!
//!     let dingtalk_state = DingTalkState::from_config(DingTalkConfig::default(), tx);
//!     let dingtalk_router = create_dingtalk_routes(dingtalk_state, None);
//!
//!     // Build server with all platform routes
//!     let server = WebhookServer::new(config)
//!         .add_router(wecom_router)
//!         .add_router(feishu_router)
//!         .add_router(dingtalk_router);
//!
//!     server.start().await?;
//!     Ok(())
//! }
//! ```
//!
//! # Routes Created
//!
//! | Platform | Default Path | Handler |
//! |----------|-------------|---------|
//! | WeCom    | `/wecom/callback` | `create_wecom_routes` |
//! | Feishu   | `/feishu/events`  | `create_feishu_routes` |
//! | DingTalk | `/dingtalk/callback` | `create_dingtalk_routes` |

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
pub use dingtalk::{create_dingtalk_routes, DingTalkState};
#[cfg(feature = "feishu")]
pub use feishu::{create_feishu_routes, FeishuState};
#[cfg(feature = "wecom")]
pub use wecom::{create_wecom_routes, WeComState};
