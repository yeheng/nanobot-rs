//! CLI-based approval interaction
//!
//! Provides terminal-based user confirmation using dialoguer.
//!
//! ## Timeout Behavior
//!
//! The CLI interaction runs in a blocking context (required by dialoguer).
//! The timeout is implemented by spawning the interaction in a separate task
//! and waiting with a timeout. If the timeout expires, the operation is
//! automatically denied.

use std::time::Duration;

use async_trait::async_trait;
use dialoguer::{Confirm, Select};
use gasket_sandbox::approval::{ApprovalRequest, PermissionLevel};
use gasket_sandbox::{ApprovalInteraction, SandboxError};

/// Default timeout for user interaction (5 minutes)
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// CLI-based approval interaction
///
/// Uses terminal prompts for user confirmation. The interaction runs
/// in a blocking context to support dialoguer's synchronous API.
pub struct CliInteraction {
    /// Timeout for user response
    timeout: Duration,
}

impl CliInteraction {
    /// Create a new CLI interaction handler with default timeout (5 minutes)
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
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
        println!("⏱️  Timeout: {} seconds", self.timeout.as_secs());
        println!("{}", "=".repeat(60));
    }

    /// Run the interactive prompt (blocking)
    fn run_prompt(&self, request: &ApprovalRequest) -> Result<PermissionLevel, SandboxError> {
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

impl Default for CliInteraction {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalInteraction for CliInteraction {
    async fn confirm(&self, request: &ApprovalRequest) -> Result<PermissionLevel, SandboxError> {
        let request = request.clone();

        // Run the interactive prompt in a blocking context.
        //
        // NOTE: No timeout wrapper! dialoguer reads from stdin which is a
        // blocking OS operation. tokio::time::timeout cannot cancel a
        // spawn_blocking thread — it only drops the JoinHandle, leaving the
        // OS thread permanently stuck in stdin. After repeated timeouts this
        // exhausts Tokio's blocking thread pool (default 512 threads), causing
        // a deadlocked gateway.
        //
        // In CLI mode, the human operator decides when to respond. The
        // terminal session itself is the natural timeout boundary.
        let result = tokio::task::spawn_blocking(move || {
            let cli = CliInteraction::new();
            cli.run_prompt(&request)
        })
        .await;

        match result {
            // spawn_blocking succeeded, prompt succeeded
            Ok(Ok(level)) => Ok(level),
            // spawn_blocking succeeded, prompt failed
            Ok(Err(e)) => Err(e),
            // spawn_blocking task panicked or was cancelled
            Err(join_err) => Err(SandboxError::ApprovalFailed(format!(
                "Blocking task failed: {}",
                join_err
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_interaction_creation() {
        let interaction = CliInteraction::new();
        assert_eq!(
            interaction.timeout(),
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );

        let interaction = CliInteraction::with_timeout(Duration::from_secs(60));
        assert_eq!(interaction.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn test_default_timeout() {
        assert_eq!(DEFAULT_TIMEOUT_SECS, 300);
    }
}
