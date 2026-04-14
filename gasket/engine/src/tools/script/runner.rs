//! Process runners for script tools.
//!
//! This module provides two execution modes for external script tools:
//! - `run_simple()`: One-shot execution with JSON input/output via stdin/stdout
//! - `run_jsonrpc()`: Bidirectional JSON-RPC 2.0 communication with request/response handling
//!
//! Both runners handle process spawning, timeout enforcement, stderr collection,
//! and result parsing with proper error reporting.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use super::dispatcher::{DispatcherContext, RpcDispatcher};
use super::manifest::ScriptManifest;
use super::rpc::{decode, encode, RpcMessage, RpcRequest, RpcResponse};

/// Result of a successful script execution.
#[derive(Debug, Clone)]
pub struct ScriptResult {
    /// Parsed JSON output from stdout
    pub output: Value,

    /// Collected stderr output
    pub stderr: String,

    /// Wall-clock duration from spawn to exit
    pub duration: Duration,
}

/// Errors that can occur during script execution.
#[derive(Debug, Error)]
pub enum ScriptError {
    /// Failed to spawn the script process.
    #[error("Failed to spawn script process: {0}")]
    SpawnFailed(String),

    /// Script execution exceeded the configured timeout.
    #[error("Script timed out after {0}s")]
    Timeout(u64),

    /// Script exited with a non-zero status code.
    #[error("Script exited with non-zero code: {0:?}")]
    NonZeroExit(Option<i32>),

    /// Script output was not valid JSON.
    #[error("Invalid script output: {0}")]
    InvalidOutput(String),

    /// I/O error during process communication.
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<tokio::io::Error> for ScriptError {
    fn from(err: tokio::io::Error) -> Self {
        ScriptError::Io(err.to_string())
    }
}

/// Run a script in simple mode (one-shot JSON input/output).
///
/// # Arguments
///
/// * `manifest` - Script manifest with runtime configuration
/// * `manifest_dir` - Directory containing the manifest (for resolving working_dir)
/// * `args` - Parameters to send to the script as JSON
/// * `timeout_secs` - Maximum execution time before killing the process
///
/// # Returns
///
/// - `Ok(ScriptResult)` - Script completed successfully with parsed output
/// - `Err(ScriptError)` - Spawn failed, timeout, non-zero exit, or invalid JSON
///
/// # Protocol
///
/// 1. Spawn process with piped stdin/stdout/stderr
/// 2. Write `args` as JSON to stdin, then close stdin
/// 3. Wait for process completion (with timeout)
/// 4. Parse stdout as JSON
/// 5. Collect stderr separately
///
/// # Example
///
/// ```rust,no_run
/// use gasket_engine::tools::script::runner::{run_simple, ScriptError};
/// use gasket_engine::tools::script::ScriptManifest;
/// use serde_json::json;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), ScriptError> {
/// # let manifest: ScriptManifest = unimplemented!();
/// let args = json!({"input": "value"});
/// let manifest_dir = Path::new("/path/to/manifest/dir");
/// let result = run_simple(&manifest, manifest_dir, &args, 30).await?;
/// println!("Output: {}", result.output);
/// # Ok(())
/// # }
/// ```
pub async fn run_simple(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
    args: &Value,
    timeout_secs: u64,
) -> Result<ScriptResult, ScriptError> {
    let start = std::time::Instant::now();

    // Spawn process with piped stdio
    let mut child = spawn_process(manifest, manifest_dir)?;

    // Write args to stdin and close it
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| ScriptError::Io("Failed to open stdin".to_string()))?;

    let args_json = serde_json::to_string(args)
        .map_err(|e| ScriptError::Io(format!("Failed to serialize args: {}", e)))?;
    stdin
        .write_all(args_json.as_bytes())
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to write to stdin: {}", e)))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to write newline to stdin: {}", e)))?;
    let _ = stdin; // Close stdin to signal EOF to the script

    // Wait for completion with timeout
    let timeout_duration = Duration::from_secs(timeout_secs);
    let result = match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(exit_status)) => exit_status,
        Ok(Err(e)) => {
            return Err(ScriptError::Io(format!(
                "Failed to wait for process: {}",
                e
            )))
        }
        Err(_) => {
            child
                .kill()
                .await
                .map_err(|e| ScriptError::Io(format!("Failed to kill timed-out process: {}", e)))?;
            return Err(ScriptError::Timeout(timeout_secs));
        }
    };

    // Check exit code
    if !result.success() {
        return Err(ScriptError::NonZeroExit(result.code()));
    }

    // Collect stdout and stderr
    let stdout = child
        .stdout
        .ok_or_else(|| ScriptError::Io("Stdout not captured".to_string()))?;

    let stderr = child
        .stderr
        .ok_or_else(|| ScriptError::Io("Stderr not captured".to_string()))?;

    // Read stdout as JSON
    let mut stdout_reader = BufReader::new(stdout);
    let mut stdout_line = String::new();
    stdout_reader
        .read_line(&mut stdout_line)
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to read stdout: {}", e)))?;

    let output: Value = serde_json::from_str(&stdout_line)
        .map_err(|e| ScriptError::InvalidOutput(format!("JSON parse error: {}", e)))?;

    // Read stderr
    let mut stderr_reader = BufReader::new(stderr);
    let mut stderr_text = String::new();
    stderr_reader
        .read_to_string(&mut stderr_text)
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to read stderr: {}", e)))?;

    Ok(ScriptResult {
        output,
        stderr: stderr_text,
        duration: start.elapsed(),
    })
}

/// Run a script in JSON-RPC mode (bidirectional communication).
///
/// # Arguments
///
/// * `manifest` - Script manifest with runtime configuration
/// * `manifest_dir` - Directory containing the manifest
/// * `args` - Initial parameters (passed to `initialize` method)
/// * `timeout_secs` - Maximum execution time
/// * `permissions` - Permissions granted to the script
/// * `dispatcher` - RPC dispatcher for handling method calls
/// * `ctx` - Dispatcher context with engine capabilities
///
/// # Returns
///
/// - `Ok(ScriptResult)` - Script completed with final result
/// - `Err(ScriptError)` - Spawn, timeout, exit, or protocol error
///
/// # Protocol
///
/// 1. Spawn process with piped stdio
/// 2. Start background task to drain stderr (prevents pipe deadlock)
/// 3. Send `initialize` request with `id: 0` (reserved for initialization)
/// 4. Enter `tokio::select!` loop with 3 branches:
///    - **Reader**: Read stdout line → decode → if Request: dispatch and send response
///    - **Writer**: Receive response from channel → write to stdin
///    - **Timeout**: Kill process on timeout
/// 5. When response with `id: 0` received, extract result and exit
/// 6. Collect stderr and return result
///
/// # Example
///
/// ```rust,no_run
/// use gasket_engine::tools::script::runner::run_jsonrpc;
/// use gasket_engine::tools::script::{ScriptManifest, dispatcher::RpcDispatcher, dispatcher::DispatcherContext};
/// use serde_json::json;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let manifest: ScriptManifest = unimplemented!();
/// # let dispatcher: RpcDispatcher = unimplemented!();
/// # let ctx: DispatcherContext = unimplemented!();
/// let manifest_dir = Path::new("/path");
/// let result = run_jsonrpc(&manifest, manifest_dir, &json!({}), 30, &[], &dispatcher, &ctx).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_jsonrpc(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
    args: &Value,
    timeout_secs: u64,
    permissions: &[super::manifest::Permission],
    dispatcher: &RpcDispatcher,
    ctx: &DispatcherContext,
) -> Result<ScriptResult, ScriptError> {
    let start = std::time::Instant::now();

    // Spawn process
    let mut child = spawn_process(manifest, manifest_dir)?;

    // Start stderr collector in background
    let stderr_collector = StderrCollector::new(child.stderr.take());

    // Split stdin/stdout for concurrent access
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ScriptError::Io("Failed to open stdin".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ScriptError::Io("Failed to open stdout".to_string()))?;

    let stdout_reader = BufReader::new(stdout);
    let mut reader_lines = stdout_reader.lines();

    // Channel for response writer (buffer size 16)
    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<RpcResponse>(16);

    // Send initialize request (id: 0 is reserved)
    let init_request = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::Number(0.into())), // Reserved id for initialization
        method: "initialize".to_string(),
        params: Some(args.clone()),
    };
    let init_msg = RpcMessage::Request(init_request);
    let init_encoded = encode(&init_msg);

    stdin
        .write_all(init_encoded.as_bytes())
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to write initialize request: {}", e)))?;
    stdin
        .flush()
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to flush stdin: {}", e)))?;

    // Track final result (id: 0 response)
    let mut final_result: Option<Value> = None;

    // Main event loop with timeout
    let timeout_duration = Duration::from_secs(timeout_secs);
    let sleep_future = tokio::time::sleep(timeout_duration);

    tokio::pin!(sleep_future);

    loop {
        tokio::select! {
            // Reader branch: read stdout and handle messages
            line_result = reader_lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        // Decode message
                        let msg = decode(&line);
                        let msg = match msg {
                            Some(m) => m,
                            None => continue, // Skip invalid/empty lines
                        };

                        match msg {
                            RpcMessage::Request(request) => {
                                // Dispatch request and send response
                                let response = dispatcher.dispatch(request, permissions, ctx).await;
                                if response_tx.send(response).await.is_err() {
                                    return Err(ScriptError::Io("Response channel closed".to_string()));
                                }
                            }
                            RpcMessage::Response(response) => {
                                // Check if this is the initialize response (id: 0)
                                if response.id == Value::Number(0.into()) {
                                    if let Some(error) = response.error {
                                        return Err(ScriptError::InvalidOutput(format!(
                                            "Initialize failed: {} (code {})",
                                            error.message, error.code
                                        )));
                                    }
                                    final_result = response.result;
                                    break; // Exit loop on initialize response
                                }
                                // Other responses are ignored (should not happen in this protocol)
                            }
                        }
                    }
                    Ok(None) => {
                        // EOF - script closed stdout
                        return Err(ScriptError::InvalidOutput("Unexpected EOF from script".to_string()));
                    }
                    Err(e) => {
                        return Err(ScriptError::Io(format!("Failed to read stdout: {}", e)));
                    }
                }
            }

            // Writer branch: write responses to stdin
            response_opt = response_rx.recv() => {
                match response_opt {
                    Some(response) => {
                        let msg = RpcMessage::Response(response);
                        let encoded = encode(&msg);
                        stdin.write_all(encoded.as_bytes()).await
                            .map_err(|e| ScriptError::Io(format!("Failed to write response: {}", e)))?;
                        stdin.flush().await
                            .map_err(|e| ScriptError::Io(format!("Failed to flush stdin: {}", e)))?;
                    }
                    None => {
                        return Err(ScriptError::Io("Response channel closed".to_string()));
                    }
                }
            }

            // Timeout branch
            _ = &mut sleep_future => {
                child.kill().await
                    .map_err(|e| ScriptError::Io(format!("Failed to kill timed-out process: {}", e)))?;
                return Err(ScriptError::Timeout(timeout_secs));
            }
        }
    }

    // Wait for child exit
    let exit_status = child
        .wait()
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to wait for process: {}", e)))?;

    if !exit_status.success() {
        return Err(ScriptError::NonZeroExit(exit_status.code()));
    }

    // Collect stderr
    let stderr = stderr_collector.collect().await;

    // Extract final result
    let output = final_result.ok_or_else(|| {
        ScriptError::InvalidOutput("No result received from initialize".to_string())
    })?;

    Ok(ScriptResult {
        output,
        stderr,
        duration: start.elapsed(),
    })
}

/// Spawn a child process from the manifest configuration.
///
/// # Arguments
///
/// * `manifest` - Script manifest with runtime configuration
/// * `manifest_dir` - Directory containing the manifest (for resolving working_dir)
///
/// # Returns
///
/// - `Ok(Child)` - Spawned process with piped stdin/stdout/stderr and kill-on-drop
/// - `Err(ScriptError)` - Failed to spawn or resolve working directory
///
/// # Configuration
///
/// - Command from `manifest.runtime.command`
/// - Args from `manifest.runtime.args`
/// - Working dir resolved relative to `manifest_dir` ("." → manifest_dir)
/// - Environment vars from `manifest.runtime.env`
/// - Pipes: stdin/stdout/stderr
/// - Kill-on-drop: true (auto-terminate on timeout/error)
fn spawn_process(manifest: &ScriptManifest, manifest_dir: &Path) -> Result<Child, ScriptError> {
    // Resolve working directory
    let working_dir = if manifest.runtime.working_dir == "." {
        manifest_dir.to_path_buf()
    } else {
        manifest_dir.join(&manifest.runtime.working_dir)
    };

    // Build command
    let mut cmd = Command::new(&manifest.runtime.command);
    cmd.args(&manifest.runtime.args)
        .current_dir(&working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Set environment variables
    for (key, value) in &manifest.runtime.env {
        cmd.env(key, value);
    }

    // Spawn process
    cmd.spawn().map_err(|e| {
        ScriptError::SpawnFailed(format!(
            "Failed to spawn '{}': {}",
            manifest.runtime.command, e
        ))
    })
}

/// Background stderr collector to prevent pipe deadlock.
///
/// When a script writes to both stdout and stderr, the OS pipe buffer
/// can fill up and cause a deadlock if stderr is not drained. This
/// helper spawns a background task to read stderr asynchronously.
struct StderrCollector {
    /// Join handle for the background task
    handle: Option<tokio::task::JoinHandle<String>>,
}

impl StderrCollector {
    /// Create a new stderr collector and spawn the background task.
    ///
    /// # Arguments
    ///
    /// * `stderr` - Optional stderr stream (None → no collection)
    fn new(stderr: Option<tokio::process::ChildStderr>) -> Self {
        let handle = stderr.map(|stream| {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut buffer = String::new();
                let result = reader.read_to_string(&mut buffer).await;
                match result {
                    Ok(_) => buffer,
                    Err(e) => {
                        tracing::warn!("Failed to read stderr: {}", e);
                        String::new()
                    }
                }
            })
        });

        Self { handle }
    }

    /// Wait for the background task to complete and return the collected stderr.
    ///
    /// # Returns
    ///
    /// Collected stderr text (empty if no stderr stream or task failed)
    async fn collect(mut self) -> String {
        match self.handle.take() {
            Some(handle) => handle.await.unwrap_or_default(),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Create a minimal manifest for testing.
    fn test_manifest(command: &str) -> (ScriptManifest, TempDir) {
        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: super::super::manifest::RuntimeConfig {
                command: command.to_string(),
                args: vec![],
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: super::super::manifest::ScriptProtocol::Simple,
            parameters: serde_json::json!({}),
            permissions: vec![],
        };
        (manifest, dir)
    }

    #[tokio::test]
    async fn test_simple_mode_cat() {
        // Use `cat` as a simple script that echoes stdin back
        let (manifest, dir) = test_manifest("cat");

        // Input JSON
        let args = json!({"hello": "world", "number": 42});

        // Run the script
        let result = run_simple(&manifest, dir.path(), &args, 5)
            .await
            .expect("Script execution failed");

        // Verify output matches input
        assert_eq!(result.output, args);
        assert_eq!(result.stderr, ""); // cat should not write to stderr
    }

    #[tokio::test]
    async fn test_simple_mode_timeout() {
        // Create a manifest with sleep command and argument
        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: super::super::manifest::RuntimeConfig {
                command: "sleep".to_string(),
                args: vec!["10".to_string()], // Sleep for 10 seconds
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: super::super::manifest::ScriptProtocol::Simple,
            parameters: serde_json::json!({}),
            permissions: vec![],
        };

        // Any input (will be ignored by sleep)
        let args = json!("unused");
        let timeout_secs = 1; // Timeout after 1 second

        // Should timeout
        let result = run_simple(&manifest, dir.path(), &args, timeout_secs).await;

        match result {
            Err(ScriptError::Timeout(t)) => {
                assert_eq!(t, timeout_secs);
            }
            other => panic!("Expected Timeout error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_spawn_fails_for_bad_command() {
        let (manifest, dir) = test_manifest("nonexistent_command_xyz123");

        let args = json!({});
        let result = run_simple(&manifest, dir.path(), &args, 5).await;

        match result {
            Err(ScriptError::SpawnFailed(_)) => {
                // Expected
            }
            other => panic!("Expected SpawnFailed error, got: {:?}", other),
        }
    }
}
