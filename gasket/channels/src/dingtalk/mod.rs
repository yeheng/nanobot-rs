//! DingTalk (钉钉) channel module
//!
//! This module provides the adapter implementation for DingTalk messaging platform.
//!
//! # Feature
//!
//! This module is enabled by the `dingtalk` feature flag.

pub mod channel;

// Re-export public API
pub use channel::{
    DingTalkAdapter, DingTalkCallbackMessage, DingTalkChannel, DingTalkConfig, DingTalkTextContent,
    DingTalkWebhookResponse,
};
