//! WeCom (企业微信) channel module
//!
//! This module provides both the channel implementation and webhook handler
//! for WeCom/WeChat Work messaging platform.
//!
//! # Components
//!
//! - [`channel`] - Channel implementation for sending/receiving messages
//! - [`webhook`] - Axum routes for handling WeCom callbacks
//! - [`crypto`] - Cryptographic helpers for signature verification and message decryption
//!
//! # Feature
//!
//! This module is enabled by the `wecom` feature flag.

pub mod channel;
pub mod crypto;
pub mod webhook;

// Re-export public API
pub use channel::{
    parse_callback_xml, WeComCallbackBody, WeComCallbackMessage, WeComCallbackQuery, WeComChannel,
    WeComConfig,
};
pub use crypto::{compute_signature, decode_aes_key, decrypt_message};
pub use webhook::{create_wecom_routes, WeComState};
