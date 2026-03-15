//! Approval manager
//!
//! Central coordinator for the approval system, combining rule storage,
//! session caching, and user interaction.

use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{
    ApprovalRequest, ApprovalRule, ApprovalSession, ExecutionContext, OperationType,
    PermissionLevel, PermissionStore, PermissionVerdict,
};
use crate::config::ApprovalConfig;
use crate::error::{Result, SandboxError};

/// Approval interaction trait
///
/// Defines how the system interacts with users for approval requests.
#[async_trait]
pub trait ApprovalInteraction: Send + Sync {
    /// Request user confirmation for an operation
    async fn confirm(&self, request: &ApprovalRequest) -> Result<PermissionLevel>;
}

/// Rule engine for matching operations against rules
pub struct RuleEngine {
    rules: Vec<ApprovalRule>,
}

impl RuleEngine {
    /// Create a new rule engine
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Set the rules
    pub fn set_rules(&mut self, rules: Vec<ApprovalRule>) {
        self.rules = rules;
    }

    /// Add a rule
    pub fn add_rule(&mut self, rule: ApprovalRule) {
        self.rules.push(rule);
    }

    /// Find a matching rule for an operation
    pub fn find_match(&self, operation: &OperationType) -> Option<&ApprovalRule> {
        self.rules
            .iter()
            .filter(|r| !r.is_expired())
            .find(|r| self.matches(&r.operation, operation))
    }

    /// Check if an operation matches a pattern
    fn matches(&self, pattern: &OperationType, operation: &OperationType) -> bool {
        match (pattern, operation) {
            (
                OperationType::Command {
                    binary: pb,
                    args: _,
                },
                OperationType::Command {
                    binary: ob,
                    args: _,
                },
            ) => self.pattern_matches(pb, ob),
            (
                OperationType::FileRead { path_pattern: pp },
                OperationType::FileRead { path_pattern: op },
            ) => self.pattern_matches(pp, op),
            (
                OperationType::FileWrite { path_pattern: pp },
                OperationType::FileWrite { path_pattern: op },
            ) => self.pattern_matches(pp, op),
            (
                OperationType::Network {
                    host_pattern: ph,
                    port: _,
                },
                OperationType::Network {
                    host_pattern: oh,
                    port: _,
                },
            ) => self.pattern_matches(ph, oh),
            (
                OperationType::EnvVar { name_pattern: pn },
                OperationType::EnvVar { name_pattern: on },
            ) => self.pattern_matches(pn, on),
            (
                OperationType::Custom {
                    category: pc,
                    name: pn,
                },
                OperationType::Custom {
                    category: oc,
                    name: on,
                },
            ) => self.pattern_matches(pc, oc) && self.pattern_matches(pn, on),
            _ => false,
        }
    }

    /// Simple glob-style pattern matching
    fn pattern_matches(&self, pattern: &str, value: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if pattern.starts_with('*') && pattern.ends_with('*') {
            let middle = &pattern[1..pattern.len() - 1];
            return value.contains(middle);
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return value.ends_with(suffix);
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return value.starts_with(prefix);
        }
        pattern == value
    }

    /// Get all rules
    pub fn rules(&self) -> &[ApprovalRule] {
        &self.rules
    }

    /// Remove a rule by ID
    pub fn remove_rule(&mut self, rule_id: uuid::Uuid) {
        self.rules.retain(|r| r.id != rule_id);
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Approval manager
///
/// Central coordinator for the approval system.
pub struct ApprovalManager {
    /// Permission store
    store: Box<dyn PermissionStore>,
    /// Rule engine
    rules: RuleEngine,
    /// Session cache
    session: ApprovalSession,
    /// User interaction handler
    interaction: Option<Arc<dyn ApprovalInteraction>>,
    /// Configuration
    config: ApprovalConfig,
}

impl ApprovalManager {
    /// Create a new approval manager
    pub fn new(store: Box<dyn PermissionStore>, config: ApprovalConfig) -> Self {
        Self {
            store,
            rules: RuleEngine::new(),
            session: ApprovalSession::new(config.session_timeout),
            interaction: None,
            config,
        }
    }

    /// Set the interaction handler
    pub fn with_interaction(mut self, interaction: Arc<dyn ApprovalInteraction>) -> Self {
        self.interaction = Some(interaction);
        self
    }

    /// Initialize by loading rules from storage
    pub async fn initialize(&mut self) -> Result<()> {
        let rules = self.store.load_rules().await?;
        self.rules.set_rules(rules);
        info!("Loaded {} approval rules", self.rules.rules().len());
        Ok(())
    }

    /// Check permission for an operation
    pub async fn check_permission(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
    ) -> PermissionVerdict {
        // 1. Check session cache for "ask_once" permissions
        if let Some(level) = self.session.check_cache(operation, context) {
            debug!("Permission found in session cache: {:?}", level);
            return match level {
                PermissionLevel::Allowed => PermissionVerdict::Allowed,
                PermissionLevel::Denied => PermissionVerdict::Denied {
                    reason: "Previously denied in this session".into(),
                },
                _ => PermissionVerdict::NeedsConfirmation {
                    request_id: uuid::Uuid::new_v4(),
                    suggested_level: level,
                },
            };
        }

        // 2. Check rules
        if let Some(rule) = self.rules.find_match(operation) {
            debug!("Found matching rule: {:?}", rule.permission);
            return match rule.permission {
                PermissionLevel::Allowed => PermissionVerdict::Allowed,
                PermissionLevel::Denied => PermissionVerdict::Denied {
                    reason: rule
                        .description
                        .clone()
                        .unwrap_or_else(|| "Denied by rule".into()),
                },
                PermissionLevel::AskOnce => {
                    // Needs confirmation, but will be cached for the session
                    PermissionVerdict::NeedsConfirmation {
                        request_id: uuid::Uuid::new_v4(),
                        suggested_level: PermissionLevel::AskOnce,
                    }
                }
                PermissionLevel::AskAlways => PermissionVerdict::NeedsConfirmation {
                    request_id: uuid::Uuid::new_v4(),
                    suggested_level: PermissionLevel::AskAlways,
                },
            };
        }

        // 3. Use default level
        let default_level = self.default_level();
        debug!("Using default permission level: {:?}", default_level);

        match default_level {
            PermissionLevel::Allowed => PermissionVerdict::Allowed,
            PermissionLevel::Denied => PermissionVerdict::Denied {
                reason: "Denied by default policy".into(),
            },
            _ => PermissionVerdict::NeedsConfirmation {
                request_id: uuid::Uuid::new_v4(),
                suggested_level: default_level,
            },
        }
    }

    /// Request approval from user
    pub async fn request_approval(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
    ) -> Result<PermissionLevel> {
        let verdict = self.check_permission(operation, context).await;

        match verdict {
            PermissionVerdict::Allowed => Ok(PermissionLevel::Allowed),
            PermissionVerdict::Denied { reason } => Err(SandboxError::PermissionDenied(reason)),
            PermissionVerdict::NeedsConfirmation {
                request_id: _,
                suggested_level,
            } => {
                // Create approval request
                let request = ApprovalRequest::new(operation.clone(), operation.description())
                    .with_suggested_level(suggested_level);

                // Get user confirmation
                if let Some(interaction) = &self.interaction {
                    let level = interaction.confirm(&request).await?;

                    // Cache if ask_once
                    if level == PermissionLevel::AskOnce {
                        self.session.cache_permission(operation, context, level);
                    }

                    Ok(level)
                } else {
                    // No interaction handler, use suggested level or deny
                    warn!("No interaction handler configured, using suggested level");
                    Ok(suggested_level)
                }
            }
        }
    }

    /// Add a new rule
    pub async fn add_rule(&self, rule: ApprovalRule) -> Result<()> {
        self.store.add_rule(&rule).await?;
        // Note: We can't modify self.rules directly here because it's not mutable
        // In a real implementation, we'd use interior mutability (RwLock)
        info!("Added approval rule: {:?}", rule.id);
        Ok(())
    }

    /// Revoke a rule
    pub async fn revoke_rule(&self, rule_id: uuid::Uuid) -> Result<()> {
        self.store.remove_rule(rule_id).await?;
        info!("Revoked approval rule: {}", rule_id);
        Ok(())
    }

    /// Get default permission level
    fn default_level(&self) -> PermissionLevel {
        self.config
            .default_level
            .parse()
            .unwrap_or(PermissionLevel::AskAlways)
    }

    /// Get all rules
    pub fn rules(&self) -> &[ApprovalRule] {
        self.rules.rules()
    }

    /// Clear session cache
    pub fn clear_session(&self) {
        self.session.clear_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_engine_pattern_matching() {
        let mut engine = RuleEngine::new();

        // Add a rule for ls command
        engine.add_rule(ApprovalRule::new(
            OperationType::command("ls"),
            PermissionLevel::Allowed,
        ));

        // Add a rule for any file read in /tmp
        engine.add_rule(ApprovalRule::new(
            OperationType::file_read("/tmp/*"),
            PermissionLevel::Allowed,
        ));

        // Test command matching
        let op = OperationType::command("ls");
        assert!(engine.find_match(&op).is_some());

        let op = OperationType::command("rm");
        assert!(engine.find_match(&op).is_none());
    }

    #[test]
    fn test_pattern_matching() {
        let engine = RuleEngine::new();

        // Exact match
        assert!(engine.pattern_matches("ls", "ls"));
        assert!(!engine.pattern_matches("ls", "rm"));

        // Wildcard match
        assert!(engine.pattern_matches("*", "anything"));
        assert!(engine.pattern_matches("/tmp/*", "/tmp/file.txt"));
        assert!(engine.pattern_matches("*.txt", "file.txt"));
        assert!(engine.pattern_matches("*test*", "my_test_file"));
    }
}
