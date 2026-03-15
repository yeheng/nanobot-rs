//! Permission store trait
//!
//! Defines the interface for permission persistence backends.

use async_trait::async_trait;

use super::ApprovalRule;
use crate::error::Result;

/// Permission store trait - supports multiple backends
#[async_trait]
pub trait PermissionStore: Send + Sync {
    /// Load all rules from storage
    async fn load_rules(&self) -> Result<Vec<ApprovalRule>>;

    /// Save all rules to storage
    async fn save_rules(&self, rules: &[ApprovalRule]) -> Result<()>;

    /// Add a single rule
    async fn add_rule(&self, rule: &ApprovalRule) -> Result<()>;

    /// Remove a rule by ID
    async fn remove_rule(&self, rule_id: uuid::Uuid) -> Result<()>;

    /// Update an existing rule
    async fn update_rule(&self, rule: &ApprovalRule) -> Result<()>;

    /// Get a rule by ID
    async fn get_rule(&self, rule_id: uuid::Uuid) -> Result<Option<ApprovalRule>>;

    /// Clear all rules
    async fn clear(&self) -> Result<()>;
}
