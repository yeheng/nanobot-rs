//! Webhook types and error definitions

use async_trait::async_trait;
use axum::{body::Body, http::Response};
use std::sync::Arc;

/// Result type for webhook operations
pub type WebhookResult<T> = Result<T, WebhookError>;

/// Error type for webhook operations
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("HTTP server error: {0}")]
    HttpError(#[from] axum::http::Error),

    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Signature verification failed")]
    SignatureVerificationFailed,

    #[error("Missing required header: {0}")]
    MissingHeader(&'static str),

    #[error("Invalid request body: {0}")]
    InvalidBody(String),

    #[error("Channel not found: {0}")]
    ChannelNotFound(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Trait for platform-specific webhook handlers.
///
/// Each messaging platform (WeCom, Feishu, DingTalk, etc.) should implement
/// this trait to handle their specific webhook callback format.
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Returns the path this handler responds to (e.g., "/wecom/callback")
    fn path(&self) -> &str;

    /// Handle a GET request (used for URL verification by some platforms)
    async fn handle_get(
        &self,
        query: axum::extract::Query<serde_json::Value>,
    ) -> WebhookResult<Response<Body>>;

    /// Handle a POST request (actual message callbacks)
    async fn handle_post(
        &self,
        headers: axum::http::HeaderMap,
        query: axum::extract::Query<serde_json::Value>,
        body: bytes::Bytes,
    ) -> WebhookResult<Response<Body>>;
}

/// A boxed webhook handler
pub type BoxedWebhookHandler = Arc<dyn WebhookHandler>;
