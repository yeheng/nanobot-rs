//! VaultStore - Secure storage for sensitive data
//!
//! Completely isolated from memory/history storage.
//! Location: ~/.nanobot/vault/secrets.json

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::VaultError;

/// A vault entry containing sensitive data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    /// Unique identifier (used in placeholder: {{vault:key}})
    pub key: String,
    /// The sensitive value
    pub value: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last usage timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
}

/// Metadata for a vault entry (excludes the value)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMetadata {
    pub key: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

impl From<&VaultEntry> for VaultMetadata {
    fn from(entry: &VaultEntry) -> Self {
        Self {
            key: entry.key.clone(),
            description: entry.description.clone(),
            created_at: entry.created_at,
            last_used: entry.last_used,
        }
    }
}

/// Vault storage for sensitive data
///
/// Storage location: ~/.nanobot/vault/secrets.json
/// Completely isolated from SQLite and Markdown files.
pub struct VaultStore {
    /// Path to the storage file
    path: PathBuf,
    /// In-memory entries with RwLock for concurrent access
    entries: RwLock<HashMap<String, VaultEntry>>,
}

impl VaultStore {
    /// Default storage path: ~/.nanobot/vault/secrets.json
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join(".nanobot")
            .join("vault")
            .join("secrets.json")
    }

    /// Create a new VaultStore with default path
    pub fn new() -> Result<Self, VaultError> {
        Self::with_path(Self::default_path())
    }

    /// Create a VaultStore with a custom path
    pub fn with_path(path: PathBuf) -> Result<Self, VaultError> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Load existing data
        let entries = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if content.trim().is_empty() {
                        HashMap::new()
                    } else {
                        match serde_json::from_str::<HashMap<String, VaultEntry>>(&content) {
                            Ok(loaded) => {
                                info!("[Vault] Loaded {} entries from {:?}", loaded.len(), path);
                                loaded
                            }
                            Err(e) => {
                                warn!("[Vault] Failed to parse vault file, starting fresh: {}", e);
                                HashMap::new()
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[Vault] Failed to read vault file, starting fresh: {}", e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            entries: RwLock::new(entries),
        })
    }

    /// Create an in-memory VaultStore (for testing)
    pub fn new_in_memory() -> Self {
        Self {
            path: PathBuf::from(":memory:"),
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Get a sensitive value by key
    ///
    /// Returns None if the key doesn't exist.
    /// Updates last_used timestamp on successful access.
    pub fn get(&self, key: &str) -> Option<String> {
        // First read to check existence and get value
        let value = {
            let entries = self.entries.read().map_err(|_| {
                warn!("[Vault] Failed to acquire read lock");
            }).ok()?;
            entries.get(key).map(|e| e.value.clone())
        };

        if value.is_some() {
            // Update last_used in a separate write operation
            if let Ok(mut entries) = self.entries.write() {
                if let Some(entry) = entries.get_mut(key) {
                    entry.last_used = Some(Utc::now());
                }
            }
        }

        value
    }

    /// Set a sensitive value
    ///
    /// Creates a new entry or updates an existing one.
    /// Preserves created_at for existing entries.
    pub fn set(
        &self,
        key: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<(), VaultError> {
        // Validate key format
        Self::validate_key(key)?;

        let mut entries = self.entries.write()
            .map_err(|e| VaultError::Lock(format!("Failed to acquire write lock: {}", e)))?;

        let created_at = entries.get(key)
            .map(|e| e.created_at)
            .unwrap_or_else(Utc::now);

        let entry = VaultEntry {
            key: key.to_string(),
            value: value.to_string(),
            description: description.map(|s| s.to_string()),
            created_at,
            last_used: None,
        };

        entries.insert(key.to_string(), entry);
        self.persist(&entries)?;

        debug!("[Vault] Set entry: {}", key);
        Ok(())
    }

    /// List all keys with metadata (values excluded)
    pub fn list_keys(&self) -> Vec<VaultMetadata> {
        match self.entries.read() {
            Ok(entries) => entries.values().map(VaultMetadata::from).collect(),
            Err(e) => {
                warn!("[Vault] Failed to acquire read lock: {}", e);
                Vec::new()
            }
        }
    }

    /// Delete an entry by key
    ///
    /// Returns true if the entry existed and was deleted.
    pub fn delete(&self, key: &str) -> Result<bool, VaultError> {
        let mut entries = self.entries.write()
            .map_err(|e| VaultError::Lock(format!("Failed to acquire write lock: {}", e)))?;

        if entries.remove(key).is_some() {
            self.persist(&entries)?;
            debug!("[Vault] Deleted entry: {}", key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Check if a key exists
    pub fn exists(&self, key: &str) -> bool {
        match self.entries.read() {
            Ok(entries) => entries.contains_key(key),
            Err(_) => false,
        }
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        match self.entries.read() {
            Ok(entries) => entries.len(),
            Err(_) => 0,
        }
    }

    /// Check if the vault is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Persist entries to disk
    fn persist(&self, entries: &HashMap<String, VaultEntry>) -> Result<(), VaultError> {
        if self.path.to_str() == Some(":memory:") {
            return Ok(());
        }

        let content = serde_json::to_string_pretty(entries)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// Validate key format
    ///
    /// Keys must be non-empty and contain only alphanumeric characters and underscores.
    fn validate_key(key: &str) -> Result<(), VaultError> {
        if key.is_empty() {
            return Err(VaultError::InvalidKey("Key cannot be empty".to_string()));
        }
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(VaultError::InvalidKey(
                "Key must contain only alphanumeric characters and underscores".to_string()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let store = VaultStore::new_in_memory();
        store.set("api_key", "secret123", Some("Test API key")).unwrap();

        let value = store.get("api_key");
        assert_eq!(value, Some("secret123".to_string()));
    }

    #[test]
    fn test_get_nonexistent() {
        let store = VaultStore::new_in_memory();
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_delete() {
        let store = VaultStore::new_in_memory();
        store.set("key", "value", None).unwrap();

        assert!(store.delete("key").unwrap());
        assert!(!store.exists("key"));
    }

    #[test]
    fn test_delete_nonexistent() {
        let store = VaultStore::new_in_memory();
        assert!(!store.delete("nonexistent").unwrap());
    }

    #[test]
    fn test_list_keys_excludes_values() {
        let store = VaultStore::new_in_memory();
        store.set("key1", "secret1", Some("Description 1")).unwrap();
        store.set("key2", "secret2", None).unwrap();

        let keys = store.list_keys();
        assert_eq!(keys.len(), 2);

        // Verify values are not included
        for meta in keys {
            // VaultMetadata doesn't have a value field
            assert!(store.get(&meta.key).is_some());
        }
    }

    #[test]
    fn test_invalid_key_empty() {
        let store = VaultStore::new_in_memory();
        let result = store.set("", "value", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_key_special_chars() {
        let store = VaultStore::new_in_memory();
        let result = store.set("key-with-dash", "value", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_key_with_underscore() {
        let store = VaultStore::new_in_memory();
        assert!(store.set("api_key_v2", "value", None).is_ok());
    }

    #[test]
    fn test_update_preserves_created_at() {
        let store = VaultStore::new_in_memory();
        store.set("key", "value1", None).unwrap();

        let original_meta = store.list_keys().into_iter().find(|m| m.key == "key").unwrap();

        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        store.set("key", "value2", Some("Updated")).unwrap();

        let updated_meta = store.list_keys().into_iter().find(|m| m.key == "key").unwrap();

        // created_at should be preserved
        assert_eq!(original_meta.created_at, updated_meta.created_at);
        // description should be updated
        assert_eq!(updated_meta.description, Some("Updated".to_string()));
    }
}
