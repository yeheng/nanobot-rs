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

    #[error("Vault is locked - password required")]
    Locked,

    #[error("Invalid password")]
    InvalidPassword,

    #[error("Vault migration error: {0}")]
    Migration(String),

    #[error("Vault file corrupted: {0}")]
    Corrupted(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_not_found() {
        let err = VaultError::NotFound("test_key".to_string());
        assert_eq!(err.to_string(), "Vault entry not found: test_key");
    }

    #[test]
    fn test_error_display_already_exists() {
        let err = VaultError::AlreadyExists("existing_key".to_string());
        assert_eq!(err.to_string(), "Vault entry already exists: existing_key");
    }

    #[test]
    fn test_error_display_invalid_key() {
        let err = VaultError::InvalidKey("key-with-dash".to_string());
        assert_eq!(err.to_string(), "Invalid key name: key-with-dash");
    }

    #[test]
    fn test_error_display_encryption() {
        let err = VaultError::Encryption("failed to encrypt".to_string());
        assert_eq!(err.to_string(), "Encryption error: failed to encrypt");
    }

    #[test]
    fn test_error_display_lock() {
        let err = VaultError::Lock("poisoned lock".to_string());
        assert_eq!(err.to_string(), "Lock error: poisoned lock");
    }

    #[test]
    fn test_error_display_locked() {
        let err = VaultError::Locked;
        assert_eq!(err.to_string(), "Vault is locked - password required");
    }

    #[test]
    fn test_error_display_invalid_password() {
        let err = VaultError::InvalidPassword;
        assert_eq!(err.to_string(), "Invalid password");
    }

    #[test]
    fn test_error_display_migration() {
        let err = VaultError::Migration("v1 to v2 failed".to_string());
        assert_eq!(err.to_string(), "Vault migration error: v1 to v2 failed");
    }

    #[test]
    fn test_error_display_corrupted() {
        let err = VaultError::Corrupted("invalid JSON".to_string());
        assert_eq!(err.to_string(), "Vault file corrupted: invalid JSON");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let vault_err: VaultError = io_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_err = serde_json::from_str::<i32>("not a number").unwrap_err();
        let vault_err: VaultError = json_err.into();
        assert!(matches!(vault_err, VaultError::Serialization(_)));
    }

    #[test]
    fn test_error_debug() {
        let err = VaultError::Locked;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Locked"));
    }
}
