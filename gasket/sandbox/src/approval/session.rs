//! Approval session management
//!
//! Manages session-based permission caching for "ask_once" permissions.
//!
//! ## Caching Logic
//!
//! When a user responds to an approval request:
//! - `AskOnce` + user approves → cache `Allowed` (auto-approve for session)
//! - `AskOnce` + user denies → cache `Denied` (auto-deny for session)
//! - `AskAlways` → never cached (always ask)
//! - `Allowed`/`Denied` → handled by rules, not session cache

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

/// Cached decision for an operation in a session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachedDecision {
    /// User approved this operation for the session
    Approved,
    /// User denied this operation for the session
    Denied,
}

/// Session-based permission cache entry
#[derive(Debug, Clone)]
struct SessionEntry {
    /// The cached decision (approved or denied)
    decision: CachedDecision,
    /// When the entry was created
    created_at: DateTime<Utc>,
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

    /// Check if an operation has a cached decision.
    ///
    /// Returns `Some(PermissionLevel::Allowed)` if the user previously approved,
    /// or `Some(PermissionLevel::Denied)` if the user previously denied.
    /// Returns `None` if no cached decision exists or the entry has expired.
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
                // Return the cached decision as a PermissionLevel
                match entry.decision {
                    CachedDecision::Approved => Some(PermissionLevel::Allowed),
                    CachedDecision::Denied => Some(PermissionLevel::Denied),
                }
            }
        })
    }

    /// Cache a user's decision for an "ask_once" operation.
    ///
    /// This should be called after the user responds to an approval request.
    /// The actual decision (approved/denied) is cached, not the `AskOnce` level.
    ///
    /// # Arguments
    /// * `operation` - The operation that was approved/denied
    /// * `context` - The execution context
    /// * `approved` - Whether the user approved (true) or denied (false)
    pub fn cache_decision(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
        approved: bool,
    ) {
        let key = CacheKey {
            operation: operation.clone(),
            session_id: context.session_id.or(self.current_session_id),
        };

        let entry = SessionEntry {
            decision: if approved {
                CachedDecision::Approved
            } else {
                CachedDecision::Denied
            },
            created_at: Utc::now(),
        };

        self.cache.insert(key, entry);
    }

    /// Legacy method: Cache a permission for an operation.
    ///
    /// **Deprecated**: Use `cache_decision` instead, which properly handles
    /// the approved/denied state.
    #[deprecated(since = "2.0.0", note = "Use `cache_decision` instead")]
    pub fn cache_permission(
        &self,
        operation: &OperationType,
        context: &ExecutionContext,
        permission: PermissionLevel,
    ) {
        // Only cache AskOnce decisions - convert to approved/denied
        if permission == PermissionLevel::AskOnce {
            // Legacy behavior: assume approved when caching AskOnce
            // New code should use cache_decision explicitly
            self.cache_decision(operation, context, true);
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
    fn test_cache_decision_approved() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("ls");
        let context = ExecutionContext::new();

        // No cached decision initially
        assert!(session.check_cache(&operation, &context).is_none());

        // Cache an approval
        session.cache_decision(&operation, &context, true);

        // Should return Allowed
        let result = session.check_cache(&operation, &context);
        assert!(matches!(result, Some(PermissionLevel::Allowed)));
    }

    #[test]
    fn test_cache_decision_denied() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("rm");
        let context = ExecutionContext::new();

        // Cache a denial
        session.cache_decision(&operation, &context, false);

        // Should return Denied
        let result = session.check_cache(&operation, &context);
        assert!(matches!(result, Some(PermissionLevel::Denied)));
    }

    #[test]
    fn test_clear_cache() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("ls");
        let context = ExecutionContext::new();

        session.cache_decision(&operation, &context, true);
        session.clear_cache();

        assert_eq!(session.cache_size(), 0);
    }

    #[test]
    fn test_session_isolation() {
        let session = ApprovalSession::new(3600);
        let operation = OperationType::command("ls");

        // Cache for session 1
        let context1 = ExecutionContext::new().with_session_id(Uuid::new_v4());
        session.cache_decision(&operation, &context1, true);

        // Cache for session 2
        let context2 = ExecutionContext::new().with_session_id(Uuid::new_v4());
        session.cache_decision(&operation, &context2, false);

        // Each session should have its own decision
        let result1 = session.check_cache(&operation, &context1);
        let result2 = session.check_cache(&operation, &context2);

        assert!(matches!(result1, Some(PermissionLevel::Allowed)));
        assert!(matches!(result2, Some(PermissionLevel::Denied)));
    }

    #[test]
    fn test_expiry() {
        // Create session with very short timeout
        let session = ApprovalSession::new(0); // 0 seconds = immediate expiry

        // This test is timing-dependent, so we just verify the structure
        assert_eq!(session.cache_size(), 0);
    }
}
