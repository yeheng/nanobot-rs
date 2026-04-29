//! Built-in approval interaction implementations.
//!
//! Re-exports the `ApprovalInteraction` trait from the approval manager
//! and provides two trivial implementations for testing and defaults.

// Re-export the canonical trait definition from the approval manager.
pub use crate::approval::ApprovalInteraction;

use crate::approval::{ApprovalRequest, PermissionLevel};
use crate::error::Result;

/// No-op interaction handler that always denies
pub struct DenyAllInteraction;

#[async_trait::async_trait]
impl ApprovalInteraction for DenyAllInteraction {
    async fn confirm(&self, _request: &ApprovalRequest) -> Result<PermissionLevel> {
        Ok(PermissionLevel::Denied)
    }
}

/// No-op interaction handler that always allows
pub struct AllowAllInteraction;

#[async_trait::async_trait]
impl ApprovalInteraction for AllowAllInteraction {
    async fn confirm(&self, _request: &ApprovalRequest) -> Result<PermissionLevel> {
        Ok(PermissionLevel::Allowed)
    }
}
