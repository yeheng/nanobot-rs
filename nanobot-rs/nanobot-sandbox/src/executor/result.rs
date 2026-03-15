//! Execution result types

use serde::{Deserialize, Serialize};

/// Result of command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Exit code (None if killed by signal)
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether the command was killed due to timeout
    pub timed_out: bool,
    /// Whether the command was killed due to resource limits
    pub resource_exceeded: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl ExecutionResult {
    /// Create a successful result
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            resource_exceeded: false,
            duration_ms: 0,
        }
    }

    /// Create a failure result
    pub fn failure(exit_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code: Some(exit_code),
            stdout: String::new(),
            stderr: stderr.into(),
            timed_out: false,
            resource_exceeded: false,
            duration_ms: 0,
        }
    }

    /// Create a timeout result
    pub fn timeout() -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: "Command timed out".to_string(),
            timed_out: true,
            resource_exceeded: false,
            duration_ms: 0,
        }
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Get combined output
    pub fn output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }

    /// Set duration
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}
