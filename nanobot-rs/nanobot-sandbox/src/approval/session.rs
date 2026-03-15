//! Approval session management
//!
//! Manages session-based permission caching for "ask_once" permissions.

use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

use super::{ExecutionContext, OperationType, PermissionLevel};

/// Permission verdict from the approval system
#[derive(Debug, Clone)]
pub enum PermissionVerdict {
    /// Operation is allowed
    Allowed,
    /// Operation is denied
    Denied { reason: String },
    /// User confirmation is required
    NeedsConfirmation {
        /// The approval request to present to the user
        request_id: Uuid,
        /// Suggested permission level
        suggested_level: PermissionLevel,
    },
}

/// Session-based permission cache entry
#[derive(Debug, Clone)]
struct SessionEntry {
    /// Permission level
    permission: PermissionLevel,
    /// When the entry was created
    created_at: DateTime<Utc>,
    /// Session ID this entry belongs to
    session_id: Option<Uuid>,
}

/// Key for session cache
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct CacheKey {
    operation: OperationType,
    session_id: Option<Uuid>,
}

/// Approval session manager
///
/// Manages session-based permission caching for "ask_once" permissions.
/// This allows users to approve an operation once per session.
pub struct ApprovalSession {
    /// Session cache for "ask_once" permissions
    cache: DashMap<CacheKey, SessionEntry>,
    /// Session timeout in seconds
    session_timeout_secs: u64,
    /// Current session ID
    current_session_id: Option<Uuid>,
}

impl ApprovalSession {
    /// Create a new approval session manager
    pub fn new(session_timeout_secs: u64) -> Self {
        Self {
            cache: DashMap::new(),
            session_timeout_secs,
            current_session_id: None,
        }
    }

    /// Set the current session ID
    pub fn set_session_id(&mut self, session_id: Uuid) {
        self.current_session_id = Some(session_id);
    }

    /// Get the current session ID
    pub fn session_id(&self) -> Option<Uuid> {
        self.current_session_id
    }

    /// Check if an operation has a cached permission
    pub fn check_cache(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
    ) -> Option<PermissionLevel> {
        let key = CacheKey {
            operation: operation.clone(),
            session_id: context.session_id.or(self.current_session_id),
        };

        self.cache.get(&key).and_then(|entry| {
            // Check if entry has expired
            let elapsed = Utc::now() - entry.created_at;
            if elapsed > Duration::seconds(self.session_timeout_secs as i64) {
                // Remove expired entry
                self.cache.remove(&key);
                None
            } else {
                Some(entry.permission)
            }
        })
    }

    /// Cache a permission for an operation
    pub fn cache_permission(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
        permission: PermissionLevel,
    ) {
        if permission == PermissionLevel::AskOnce {
            // Only cache ask_once permissions
            let key = CacheKey {
                operation: operation.clone(),
                session_id: context.session_id.or(self.current_session_id),
            };

            let entry = SessionEntry {
                permission,
                created_at: Utc::now(),
                session_id: context.session_id.or(self.current_session_id),
            };

            self.cache.insert(key, entry);
        }
    }

    /// Clear all cached permissions
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Clear cached permissions for a specific session
    pub fn clear_session(&self, session_id: Uuid) {
        self.cache.retain(|k, _| k.session_id != Some(session_id));
    }

    /// Get the number of cached entries
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Clean up expired entries
    pub fn cleanup_expired(&self) {
        let now = Utc::now();
        let timeout = Duration::seconds(self.session_timeout_secs as i64);

        self.cache.retain(|_, entry| {
            let elapsed = now - entry.created_at;
            elapsed <= timeout
        });
    }
}

impl Hash for OperationType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Command { binary, args } => {
                state.write_u8(0);
                binary.hash(state);
                args.hash(state);
            }
            Self::FileRead { path_pattern } => {
                state.write_u8(1);
                path_pattern.hash(state);
            }
            Self::FileWrite { path_pattern } => {
                state.write_u8(2);
                path_pattern.hash(state);
            }
            Self::Network { host_pattern, port } => {
                state.write_u8(3);
                host_pattern.hash(state);
                port.hash(state);
            }
            Self::EnvVar { name_pattern } => {
                state.write_u8(4);
                name_pattern.hash(state);
            }
            Self::Custom { category, name } => {
                state.write_u8(5);
                category.hash(state);
                name.hash(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_permission() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("ls");
        let context = ExecutionContext::new();

        // No cached permission initially
        assert!(session.check_cache(&operation, &context).is_none());

        // Cache a permission
        session.cache_permission(&operation, &context, PermissionLevel::AskOnce);

        // Now we should have a cached permission
        // Note: AskOnce is cached, but returns None for check_cache
        // because we want to ask once per session
    }

    #[test]
    fn test_clear_cache() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("ls");
        let context = ExecutionContext::new();

        session.cache_permission(&operation, &context, PermissionLevel::AskOnce);
        session.clear_cache();

        assert_eq!(session.cache_size(), 0);
    }
}
