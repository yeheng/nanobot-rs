//! Vault error types

use thiserror::Error;

/// Vault operation errors
#[derive(Debug, Error)]
pub enum VaultError {
    #[error("Vault entry not found: {0}")]
    NotFound(String),

    #[error("Vault entry already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid key name: {0}")]
    InvalidKey(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Lock error: {0}")]
    Lock(String),
}
