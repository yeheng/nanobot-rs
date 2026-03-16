//! User interaction for approval requests
//!
//! Provides CLI and WebSocket-based interaction for user confirmations.

mod cli;

pub use cli::CliInteraction;

use async_trait::async_trait;

use crate::approval::{ApprovalRequest, PermissionLevel};
use crate::error::Result;

/// Approval interaction trait
#[async_trait]
pub trait ApprovalInteraction: Send + Sync {
    /// Request user confirmation for an operation
    async fn confirm(&self, request: &ApprovalRequest) -> Result<PermissionLevel>;
}

/// No-op interaction handler that always denies
pub struct DenyAllInteraction;

#[async_trait]
impl ApprovalInteraction for DenyAllInteraction {
    async fn confirm(&self, _request: &ApprovalRequest) -> Result<PermissionLevel> {
        Ok(PermissionLevel::Denied)
    }
}

/// No-op interaction handler that always allows
pub struct AllowAllInteraction;

#[async_trait]
impl ApprovalInteraction for AllowAllInteraction {
    async fn confirm(&self, _request: &ApprovalRequest) -> Result<PermissionLevel> {
        Ok(PermissionLevel::Allowed)
    }
}
