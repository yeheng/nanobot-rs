//! Webhook HTTP server for receiving callbacks from messaging platforms
//!
//! This module provides the core HTTP server infrastructure and common utilities
//! for handling webhook callbacks. Platform-specific webhook handlers are
//! provided in their respective platform modules:
//!
//! - `crate::dingtalk::webhook` - DingTalk webhook handler
//! - `crate::feishu::webhook` - Feishu webhook handler
//! - `crate::wecom::webhook` - WeCom webhook handler
//!
//! # Example
//!
//! ```ignore
//! use gasket_channels::webhook::{WebhookServer, WebhookConfig};
//! use gasket_channels::dingtalk::webhook::{create_dingtalk_routes, DingTalkState};
//! use gasket_channels::feishu::webhook::{create_feishu_routes, FeishuState};
//! use gasket_channels::wecom::webhook::{create_wecom_routes, WeComState};
//! use gasket_channels::dingtalk::DingTalkConfig;
//! use gasket_channels::feishu::FeishuConfig;
//! use gasket_channels::wecom::WeComConfig;
//! use gasket_types::InboundMessage;
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
//!     let dingtalk_state = DingTalkState::from_config(DingTalkConfig::default(), tx.clone());
//!     let dingtalk_router = create_dingtalk_routes(dingtalk_state, None);
//!
//!     let feishu_state = FeishuState::from_config(FeishuConfig::default(), tx.clone());
//!     let feishu_router = create_feishu_routes(feishu_state, None);
//!
//!     let wecom_state = WeComState::from_config(WeComConfig::default(), tx);
//!     let wecom_router = create_wecom_routes(wecom_state, None);
//!
//!     // Build server with all platform routes
//!     let server = WebhookServer::new(config)
//!         .add_router(dingtalk_router)
//!         .add_router(feishu_router)
//!         .add_router(wecom_router);
//!
//!     server.start().await?;
//!     Ok(())
//! }
//! ```
//!
//! # Routes Created
//!
//! | Platform | Default Path | Handler Module |
//! |----------|-------------|----------------|
//! | DingTalk | `/dingtalk/callback` | `crate::dingtalk::webhook` |
//! | Feishu   | `/feishu/events`  | `crate::feishu::webhook` |
//! | WeCom    | `/wecom/callback` | `crate::wecom::webhook` |

pub mod handlers;
mod server;
mod types;

pub use server::{WebhookConfig, WebhookServer};
pub use types::{WebhookError, WebhookResult};
