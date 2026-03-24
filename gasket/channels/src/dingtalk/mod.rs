//! DingTalk (钉钉) channel module
//!
//! This module provides both the channel implementation and webhook handler
//! for DingTalk messaging platform.
//!
//! # Components
//!
//! - [`channel`] - Channel implementation for sending/receiving messages
//! - [`webhook`] - Axum routes for handling DingTalk callbacks
//!
//! # Feature
//!
//! This module is enabled by the `dingtalk` feature flag.

pub mod channel;
pub mod webhook;

// Re-export public API
pub use channel::{
    DingTalkCallbackMessage, DingTalkChannel, DingTalkConfig, DingTalkTextContent,
    DingTalkWebhookResponse, send_message_stateless,
};
pub use webhook::{create_dingtalk_routes, DingTalkState};
