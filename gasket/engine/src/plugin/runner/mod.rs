//! Process runners for plugins.
//!
//! This module provides two execution modes for external plugins:
//! - `run_simple()`: One-shot execution with JSON input/output via stdin/stdout
//! - `run_jsonrpc()`: Bidirectional JSON-RPC 2.0 communication

mod daemon;
mod jsonrpc;
mod simple;

pub use daemon::JsonRpcDaemon;
pub use jsonrpc::run_jsonrpc;
pub use simple::run_simple;

use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

/// Result of a successful script execution.
#[derive(Debug, Clone)]
pub struct PluginResult {
    /// Parsed JSON output from stdout
    pub output: Value,

    /// Collected stderr output
    pub stderr: String,

    /// Wall-clock duration from spawn to exit
    pub duration: Duration,
}

/// Errors that can occur during script execution.
#[derive(Debug, Error)]
pub enum PluginError {
    /// Failed to spawn the script process.
    #[error("Failed to spawn script process: {0}")]
    SpawnFailed(String),

    /// Plugin execution exceeded the configured timeout.
    #[error("Plugin timed out after {0}s")]
    Timeout(u64),

    /// Plugin exited with a non-zero status code.
    #[error("Plugin exited with non-zero code: {0:?}")]
    NonZeroExit(Option<i32>),

    /// Plugin output was not valid JSON.
    #[error("Invalid script output: {0}")]
    InvalidOutput(String),

    /// I/O error during process communication.
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<tokio::io::Error> for PluginError {
    fn from(err: tokio::io::Error) -> Self {
        PluginError::Io(err.to_string())
    }
}
