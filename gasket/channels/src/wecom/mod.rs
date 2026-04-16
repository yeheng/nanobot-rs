//! WeCom (企业微信) channel module
//!
//! This module provides the adapter implementation for WeCom/WeChat Work messaging platform.
//!
//! # Components
//!
//! - [`channel`] - Adapter implementation for sending/receiving messages
//! - [`crypto`] - Cryptographic helpers for signature verification and message decryption
//!
//! # Feature
//!
//! This module is enabled by the `wecom` feature flag.

pub mod channel;
pub mod crypto;

// Re-export public API
pub use channel::{
    parse_callback_xml, WeComAdapter, WeComCallbackBody, WeComCallbackMessage, WeComCallbackQuery,
    WeComChannel, WeComConfig,
};
pub use crypto::{compute_signature, decode_aes_key, decrypt_message};
