//! JSON file-based permission store
//!
//! Provides simple file-based persistence for approval rules.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{ApprovalRule, PermissionStore};
use crate::error::{Result, SandboxError};

/// JSON file-based permission store.
///
/// Mutations are serialized through an internal `Mutex` and written
/// atomically (write-to-tempfile + rename) so concurrent `add_rule` /
/// `remove_rule` / `update_rule` calls cannot lose updates or leave the
/// store in a half-written state.
pub struct JsonPermissionStore {
    path: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl JsonPermissionStore {
    /// Create a new JSON store at the given path
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Create a store in the default location
    pub fn default_location() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".gasket")))
            .ok_or_else(|| SandboxError::StoreError("Cannot determine config directory".into()))?;

        let path = config_dir.join("approval_rules.json");
        Ok(Self::new(path))
    }

    /// Create a temporary store for testing
    #[cfg(test)]
    pub fn new_temp() -> Result<Self> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| SandboxError::StoreError(format!("Failed to create temp dir: {}", e)))?;
        let path = temp_dir.path().join("test_rules.json");
        Ok(Self::new(path))
    }

    async fn ensure_parent_dir(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    SandboxError::StoreError(format!("Failed to create directory: {}", e))
                })?;
            }
        }
        Ok(())
    }

    /// Save rules atomically. Caller must hold `write_lock`.
    /// Writes to a sibling tempfile, then renames over the destination so
    /// readers never see a partially written file.
    async fn save_rules_locked(&self, rules: &[ApprovalRule]) -> Result<()> {
        self.ensure_parent_dir().await?;

        let content = serde_json::to_string_pretty(rules)
            .map_err(|e| SandboxError::StoreError(format!("Failed to serialize rules: {}", e)))?;

        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, content).await.map_err(|e| {
            SandboxError::StoreError(format!("Failed to write temp rules file: {}", e))
        })?;
        fs::rename(&tmp, &self.path).await.map_err(|e| {
            SandboxError::StoreError(format!("Failed to rename rules file: {}", e))
        })?;

        debug!("Saved {} rules to {:?}", rules.len(), self.path);
        Ok(())
    }
}

#[async_trait]
impl PermissionStore for JsonPermissionStore {
    async fn load_rules(&self) -> Result<Vec<ApprovalRule>> {
        if !self.path.exists() {
            debug!("Rules file does not exist, returning empty list");
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.path)
            .await
            .map_err(|e| SandboxError::StoreError(format!("Failed to read rules file: {}", e)))?;

        if content.trim().is_empty() {
            return Ok(Vec::new());
        }

        let rules: Vec<ApprovalRule> = serde_json::from_str(&content)
            .map_err(|e| SandboxError::StoreError(format!("Failed to parse rules: {}", e)))?;

        // Filter out expired rules
        let active_rules: Vec<_> = rules.into_iter().filter(|r| !r.is_expired()).collect();

        debug!(
            "Loaded {} active rules from {:?}",
            active_rules.len(),
            self.path
        );
        Ok(active_rules)
    }

    async fn save_rules(&self, rules: &[ApprovalRule]) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.save_rules_locked(rules).await
    }

    async fn add_rule(&self, rule: &ApprovalRule) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut rules = self.load_rules().await?;
        rules.push(rule.clone());
        self.save_rules_locked(&rules).await
    }

    async fn remove_rule(&self, rule_id: uuid::Uuid) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut rules = self.load_rules().await?;
        let initial_len = rules.len();
        rules.retain(|r| r.id != rule_id);

        if rules.len() == initial_len {
            warn!("Rule {} not found for removal", rule_id);
        }

        self.save_rules_locked(&rules).await
    }

    async fn update_rule(&self, rule: &ApprovalRule) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut rules = self.load_rules().await?;
        let found = rules.iter_mut().find(|r| r.id == rule.id);

        if let Some(existing) = found {
            *existing = rule.clone();
            self.save_rules_locked(&rules).await
        } else {
            warn!("Rule {} not found for update, adding as new", rule.id);
            rules.push(rule.clone());
            self.save_rules_locked(&rules).await
        }
    }

    async fn get_rule(&self, rule_id: uuid::Uuid) -> Result<Option<ApprovalRule>> {
        let rules = self.load_rules().await?;
        Ok(rules.into_iter().find(|r| r.id == rule_id))
    }

    async fn clear(&self) -> Result<()> {
        self.save_rules(&[]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{OperationType, PermissionLevel};

    #[tokio::test]
    async fn test_store_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("test_rules.json");
        let store = JsonPermissionStore::new(&path);

        // Create a rule
        let rule = ApprovalRule::new(OperationType::command("ls"), PermissionLevel::Allowed);

        // Add the rule
        store.add_rule(&rule).await.unwrap();

        // Load rules
        let rules = store.load_rules().await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].operation, OperationType::command("ls"));

        // Remove the rule
        store.remove_rule(rule.id).await.unwrap();
        let rules = store.load_rules().await.unwrap();
        assert!(rules.is_empty());
    }
}
