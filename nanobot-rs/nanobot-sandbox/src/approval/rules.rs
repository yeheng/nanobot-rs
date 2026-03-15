//! Permission levels and operation types
//!
//! Defines the core types for the approval system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;
use uuid::Uuid;

/// Permission level for operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PermissionLevel {
    /// Completely denied - operation is never allowed
    Denied,
    /// Always ask for confirmation
    #[default]
    AskAlways,
    /// Ask once per session, then auto-approve
    AskOnce,
    /// Always allowed without confirmation
    Allowed,
}

impl fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PermissionLevel::Denied => write!(f, "denied"),
            PermissionLevel::AskAlways => write!(f, "ask_always"),
            PermissionLevel::AskOnce => write!(f, "ask_once"),
            PermissionLevel::Allowed => write!(f, "allowed"),
        }
    }
}

impl std::str::FromStr for PermissionLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "denied" => Ok(Self::Denied),
            "ask_always" | "ask-always" | "askalways" => Ok(Self::AskAlways),
            "ask_once" | "ask-once" | "askonce" => Ok(Self::AskOnce),
            "allowed" => Ok(Self::Allowed),
            _ => Err(format!("Invalid permission level: {}", s)),
        }
    }
}

/// Operation type for permission checks
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OperationType {
    /// Execute a command
    Command {
        /// Binary/command name
        binary: String,
        /// Full command string
        args: Option<String>,
    },
    /// Read a file
    FileRead {
        /// Path pattern (glob-style)
        path_pattern: String,
    },
    /// Write a file
    FileWrite {
        /// Path pattern (glob-style)
        path_pattern: String,
    },
    /// Network access
    Network {
        /// Host pattern (glob-style)
        host_pattern: String,
        /// Port (optional)
        port: Option<u16>,
    },
    /// Environment variable access
    EnvVar {
        /// Variable name pattern
        name_pattern: String,
    },
    /// Custom operation type
    Custom {
        /// Operation category
        category: String,
        /// Operation name
        name: String,
    },
}

impl OperationType {
    /// Create a command operation
    pub fn command(binary: impl Into<String>) -> Self {
        Self::Command {
            binary: binary.into(),
            args: None,
        }
    }

    /// Create a command operation with arguments
    pub fn command_with_args(binary: impl Into<String>, args: impl Into<String>) -> Self {
        Self::Command {
            binary: binary.into(),
            args: Some(args.into()),
        }
    }

    /// Create a file read operation
    pub fn file_read(path: impl Into<String>) -> Self {
        Self::FileRead {
            path_pattern: path.into(),
        }
    }

    /// Create a file write operation
    pub fn file_write(path: impl Into<String>) -> Self {
        Self::FileWrite {
            path_pattern: path.into(),
        }
    }

    /// Create a network operation
    pub fn network(host: impl Into<String>) -> Self {
        Self::Network {
            host_pattern: host.into(),
            port: None,
        }
    }

    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            Self::Command { binary, args } => {
                if let Some(args) = args {
                    format!("Execute command: {} {}", binary, args)
                } else {
                    format!("Execute command: {}", binary)
                }
            }
            Self::FileRead { path_pattern } => format!("Read file: {}", path_pattern),
            Self::FileWrite { path_pattern } => format!("Write file: {}", path_pattern),
            Self::Network { host_pattern, port } => {
                if let Some(port) = port {
                    format!("Network access: {}:{}", host_pattern, port)
                } else {
                    format!("Network access: {}", host_pattern)
                }
            }
            Self::EnvVar { name_pattern } => format!("Access env var: {}", name_pattern),
            Self::Custom { category, name } => format!("{}: {}", category, name),
        }
    }
}

/// Condition for approval rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Condition type
    pub condition_type: ConditionType,
    /// Whether to negate the condition
    #[serde(default)]
    pub negate: bool,
}

/// Types of conditions for approval rules
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConditionType {
    /// Time-based condition
    TimeRange {
        /// Start hour (0-23)
        start_hour: u8,
        /// End hour (0-23)
        end_hour: u8,
    },
    /// Working directory condition
    WorkingDir {
        /// Path pattern
        pattern: String,
    },
    /// User condition
    User {
        /// User name pattern
        pattern: String,
    },
    /// Session condition
    Session {
        /// Session ID
        session_id: Uuid,
    },
    /// Custom condition
    Custom {
        /// Condition name
        name: String,
        /// Condition value
        value: String,
    },
}

/// Approval rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRule {
    /// Unique rule ID
    pub id: Uuid,
    /// Operation this rule applies to
    pub operation: OperationType,
    /// Permission level for this rule
    pub permission: PermissionLevel,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Expiration timestamp (optional)
    pub expires_at: Option<DateTime<Utc>>,
    /// Additional conditions
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Description/comment
    #[serde(default)]
    pub description: Option<String>,
    /// Source of the rule (user, system, auto-learned)
    #[serde(default)]
    pub source: RuleSource,
}

impl ApprovalRule {
    /// Create a new approval rule
    pub fn new(operation: OperationType, permission: PermissionLevel) -> Self {
        Self {
            id: Uuid::new_v4(),
            operation,
            permission,
            created_at: Utc::now(),
            expires_at: None,
            conditions: Vec::new(),
            description: None,
            source: RuleSource::User,
        }
    }

    /// Set expiration
    pub fn with_expiration(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Set description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a condition
    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// Check if the rule has expired
    pub fn is_expired(&self) -> bool {
        self.expires_at.map(|exp| exp < Utc::now()).unwrap_or(false)
    }
}

/// Source of an approval rule
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// User-created rule
    #[default]
    User,
    /// System-generated rule
    System,
    /// Auto-learned from user behavior
    AutoLearned,
    /// Imported from config
    Imported,
}

impl std::fmt::Display for RuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSource::User => write!(f, "user"),
            RuleSource::System => write!(f, "system"),
            RuleSource::AutoLearned => write!(f, "auto_learned"),
            RuleSource::Imported => write!(f, "imported"),
        }
    }
}

impl std::str::FromStr for RuleSource {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            "auto_learned" | "autolearned" => Ok(Self::AutoLearned),
            "imported" => Ok(Self::Imported),
            _ => Err(format!("Invalid rule source: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_level_parse() {
        assert_eq!("denied".parse(), Ok(PermissionLevel::Denied));
        assert_eq!("ask_always".parse(), Ok(PermissionLevel::AskAlways));
        assert_eq!("ask_once".parse(), Ok(PermissionLevel::AskOnce));
        assert_eq!("allowed".parse(), Ok(PermissionLevel::Allowed));
    }

    #[test]
    fn test_operation_type_description() {
        let op = OperationType::command("ls");
        assert_eq!(op.description(), "Execute command: ls");

        let op = OperationType::file_read("/etc/passwd");
        assert_eq!(op.description(), "Read file: /etc/passwd");
    }

    #[test]
    fn test_rule_expiration() {
        let rule = ApprovalRule::new(OperationType::command("ls"), PermissionLevel::Allowed);
        assert!(!rule.is_expired());

        let expired_rule =
            ApprovalRule::new(OperationType::command("rm"), PermissionLevel::Allowed)
                .with_expiration(Utc::now() - chrono::Duration::hours(1));
        assert!(expired_rule.is_expired());
    }
}
