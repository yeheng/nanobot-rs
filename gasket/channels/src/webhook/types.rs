//! Webhook error definitions

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
