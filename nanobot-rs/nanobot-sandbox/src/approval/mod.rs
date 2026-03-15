//! Approval system for sandbox operations
//!
//! Provides a comprehensive permission management system with:
//! - Multiple permission levels (denied, ask_always, ask_once, allowed)
//! - Persistent rule storage (JSON files)
//! - Session-based caching
//! - Interactive confirmation (CLI or WebSocket)

mod manager;
mod rules;
mod session;
mod store;
mod store_json;

pub use manager::{ApprovalInteraction, ApprovalManager, RuleEngine};
pub use rules::{ApprovalRule, Condition, OperationType, PermissionLevel, RuleSource};
pub use session::{ApprovalSession, PermissionVerdict};
pub use store::PermissionStore;
pub use store_json::JsonPermissionStore;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Approval request sent to user for confirmation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique request ID
    pub id: Uuid,
    /// Operation being requested
    pub operation: OperationType,
    /// Human-readable description of the operation
    pub description: String,
    /// Context information (working directory, environment, etc.)
    pub context: HashMap<String, String>,
    /// Risk assessment (0-100)
    pub risk_score: u8,
    /// Suggested permission level
    pub suggested_level: PermissionLevel,
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ApprovalRequest {
    /// Create a new approval request
    pub fn new(operation: OperationType, description: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            operation,
            description: description.into(),
            context: HashMap::new(),
            risk_score: 0,
            suggested_level: PermissionLevel::AskAlways,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Add context information
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    /// Set risk score
    pub fn with_risk_score(mut self, score: u8) -> Self {
        self.risk_score = score.min(100);
        self
    }

    /// Set suggested permission level
    pub fn with_suggested_level(mut self, level: PermissionLevel) -> Self {
        self.suggested_level = level;
        self
    }
}

/// Response to an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    /// Request ID being responded to
    pub request_id: Uuid,
    /// Granted permission level
    pub permission: PermissionLevel,
    /// Optional reason for the decision
    pub reason: Option<String>,
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ApprovalResponse {
    /// Create a new approval response
    pub fn new(request_id: Uuid, permission: PermissionLevel) -> Self {
        Self {
            request_id,
            permission,
            reason: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Add a reason
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Execution context for permission checks
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Working directory
    pub working_dir: Option<std::path::PathBuf>,
    /// User who initiated the request
    pub user: Option<String>,
    /// Session ID
    pub session_id: Option<Uuid>,
    /// Agent ID (if applicable)
    pub agent_id: Option<String>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl ExecutionContext {
    /// Create a new execution context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Set user
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set session ID
    pub fn with_session_id(mut self, id: Uuid) -> Self {
        self.session_id = Some(id);
        self
    }
}
