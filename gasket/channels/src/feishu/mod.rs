//! Feishu (飞书) channel module
//!
//! This module provides the adapter implementation for Feishu/Lark messaging platform.
//!
//! # Feature
//!
//! This module is enabled by the `feishu` feature flag.

pub mod channel;

// Re-export public API
pub use channel::{
    FeishuAdapter, FeishuChallenge, FeishuChallengeResponse, FeishuChannel, FeishuConfig,
    FeishuEvent, FeishuEventData, FeishuMention, FeishuMentionId, FeishuMessage, FeishuSender,
    FeishuSenderId, FeishuTextContent,
};
