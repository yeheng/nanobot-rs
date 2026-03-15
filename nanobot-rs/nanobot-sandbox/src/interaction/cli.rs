//! CLI-based approval interaction
//!
//! Provides terminal-based user confirmation using dialoguer.

use std::time::Duration;

use async_trait::async_trait;
use dialoguer::{Confirm, Select};

use super::ApprovalInteraction;
use crate::approval::{ApprovalRequest, PermissionLevel};
use crate::error::{Result, SandboxError};

/// CLI-based approval interaction
pub struct CliInteraction {
    /// Timeout for user response
    timeout: Duration,
}

impl CliInteraction {
    /// Create a new CLI interaction handler
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(300), // 5 minutes default
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn display_request(&self, request: &ApprovalRequest) {
        println!("\n{}", "=".repeat(60));
        println!("🔐 Approval Request");
        println!("{}", "=".repeat(60));
        println!("\n📋 Operation: {}", request.description);
        println!("📊 Risk Score: {}/100", request.risk_score);

        if !request.context.is_empty() {
            println!("\n📎 Context:");
            for (key, value) in &request.context {
                println!("   {}: {}", key, value);
            }
        }

        println!("\n💡 Suggested: {}", request.suggested_level);
        println!("{}", "=".repeat(60));
    }
}

impl Default for CliInteraction {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalInteraction for CliInteraction {
    async fn confirm(&self, request: &ApprovalRequest) -> Result<PermissionLevel> {
        self.display_request(request);

        // First, ask if the user wants to allow this operation
        let allow = Confirm::new()
            .with_prompt("Allow this operation?")
            .default(false)
            .interact()
            .map_err(|e| {
                SandboxError::ApprovalFailed(format!("Failed to get user input: {}", e))
            })?;

        if !allow {
            println!("❌ Operation denied.");
            return Ok(PermissionLevel::Denied);
        }

        // Ask about the scope of the permission
        let options = [
            "Once (ask again next time)",
            "For this session",
            "Always (create a permanent rule)",
        ];

        let selection = Select::new()
            .with_prompt("Remember this decision?")
            .items(&options)
            .default(0)
            .interact()
            .map_err(|e| {
                SandboxError::ApprovalFailed(format!("Failed to get user input: {}", e))
            })?;

        let level = match selection {
            0 => PermissionLevel::AskAlways,
            1 => PermissionLevel::AskOnce,
            2 => PermissionLevel::Allowed,
            _ => PermissionLevel::AskAlways,
        };

        println!("✅ Permission granted: {}", level);
        Ok(level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_interaction_creation() {
        let interaction = CliInteraction::new();
        assert_eq!(interaction.timeout, Duration::from_secs(300));

        let interaction = CliInteraction::with_timeout(Duration::from_secs(60));
        assert_eq!(interaction.timeout, Duration::from_secs(60));
    }
}
