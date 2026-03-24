//! Feishu (飞书) channel module
//!
//! This module provides both the channel implementation and webhook handler
//! for Feishu/Lark messaging platform.
//!
//! # Components
//!
//! - [`channel`] - Channel implementation for sending/receiving messages
//! - [`webhook`] - Axum routes for handling Feishu callbacks
//!
//! # Feature
//!
//! This module is enabled by the `feishu` feature flag.

pub mod channel;
pub mod webhook;

// Re-export public API
pub use channel::{
    FeishuChallenge, FeishuChallengeResponse, FeishuChannel, FeishuConfig, FeishuEvent,
    FeishuEventData, FeishuMessage, FeishuMention, FeishuMentionId, FeishuSender, FeishuSenderId,
    FeishuTextContent, send_text_stateless,
};
pub use webhook::{create_feishu_routes, FeishuState};
