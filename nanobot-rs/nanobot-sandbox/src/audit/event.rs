//! Audit event types
//!
//! Defines the structure of audit events for logging.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Audit event type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEventType {
    /// Command execution started
    CommandStart {
        /// Command string
        command: String,
        /// Working directory
        working_dir: String,
    },
    /// Command execution completed
    CommandEnd {
        /// Command string
        command: String,
        /// Exit code
        exit_code: Option<i32>,
        /// Duration in milliseconds
        duration_ms: u64,
        /// Whether the command timed out
        timed_out: bool,
    },
    /// Permission granted
    PermissionGranted {
        /// Operation type
        operation: String,
        /// Permission level
        level: String,
    },
    /// Permission denied
    PermissionDenied {
        /// Operation type
        operation: String,
        /// Reason for denial
        reason: String,
    },
    /// Rule added
    RuleAdded {
        /// Rule ID
        rule_id: Uuid,
        /// Operation pattern
        operation: String,
        /// Permission level
        level: String,
    },
    /// Rule removed
    RuleRemoved {
        /// Rule ID
        rule_id: Uuid,
    },
    /// Sandbox backend changed
    BackendChanged {
        /// Old backend
        old_backend: String,
        /// New backend
        new_backend: String,
    },
    /// Security event
    SecurityEvent {
        /// Event category
        category: String,
        /// Event description
        description: String,
        /// Severity: info, warning, error, critical
        severity: String,
    },
    /// Configuration changed
    ConfigChanged {
        /// Configuration key
        key: String,
        /// Old value (if available)
        old_value: Option<String>,
        /// New value (if available)
        new_value: Option<String>,
    },
}

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID
    pub id: Uuid,
    /// Event timestamp
    pub timestamp: DateTime<Utc>,
    /// Event type
    #[serde(flatten)]
    pub event_type: AuditEventType,
    /// Session ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    /// User who triggered the event
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Agent ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type,
            session_id: None,
            user: None,
            agent_id: None,
            metadata: HashMap::new(),
        }
    }

    /// Add session ID
    pub fn with_session_id(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Add user
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Add agent ID
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Create a command start event
    pub fn command_start(command: impl Into<String>, working_dir: impl Into<String>) -> Self {
        Self::new(AuditEventType::CommandStart {
            command: command.into(),
            working_dir: working_dir.into(),
        })
    }

    /// Create a command end event
    pub fn command_end(
        command: impl Into<String>,
        exit_code: Option<i32>,
        duration_ms: u64,
        timed_out: bool,
    ) -> Self {
        Self::new(AuditEventType::CommandEnd {
            command: command.into(),
            exit_code,
            duration_ms,
            timed_out,
        })
    }

    /// Create a permission granted event
    pub fn permission_granted(operation: impl Into<String>, level: impl Into<String>) -> Self {
        Self::new(AuditEventType::PermissionGranted {
            operation: operation.into(),
            level: level.into(),
        })
    }

    /// Create a permission denied event
    pub fn permission_denied(operation: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(AuditEventType::PermissionDenied {
            operation: operation.into(),
            reason: reason.into(),
        })
    }

    /// Create a security event
    pub fn security_event(
        category: impl Into<String>,
        description: impl Into<String>,
        severity: impl Into<String>,
    ) -> Self {
        Self::new(AuditEventType::SecurityEvent {
            category: category.into(),
            description: description.into(),
            severity: severity.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = AuditEvent::command_start("ls -la", "/home/user");
        assert!(event.session_id.is_none());

        let event = event.with_session_id(Uuid::new_v4());
        assert!(event.session_id.is_some());
    }

    #[test]
    fn test_event_serialization() {
        let event = AuditEvent::command_end("ls", Some(0), 100, false);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"command_end\""));
    }
}
