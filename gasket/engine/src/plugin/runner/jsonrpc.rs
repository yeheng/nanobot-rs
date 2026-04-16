//! JSON-RPC mode runner for plugins.
//!
//! Bidirectional JSON-RPC 2.0 communication with request/response handling.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::plugin::dispatcher::{DispatcherContext, RpcDispatcher};
use crate::plugin::manifest::{Permission, PluginManifest};
use crate::plugin::rpc::{decode, encode, RpcMessage, RpcRequest, RpcResponse, MAX_MESSAGE_SIZE};
use crate::plugin::runner::{PluginError, PluginResult};

/// Background stderr collector to prevent pipe deadlock.
struct StderrCollector {
    handle: Option<tokio::task::JoinHandle<String>>,
}

impl StderrCollector {
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

    async fn collect(mut self) -> String {
        match self.handle.take() {
            Some(handle) => handle.await.unwrap_or_default(),
            None => String::new(),
        }
    }
}

/// Run a script in JSON-RPC mode (bidirectional communication).
///
/// # Arguments
///
/// * `manifest` - Plugin manifest with runtime configuration
/// * `manifest_dir` - Directory containing the manifest
/// * `args` - Initial parameters (passed to `initialize` method)
/// * `timeout_secs` - Maximum execution time
/// * `permissions` - Permissions granted to the script
/// * `dispatcher` - RPC dispatcher for handling method calls
/// * `ctx` - Dispatcher context with engine capabilities
///
/// # Returns
///
/// - `Ok(PluginResult)` - Plugin completed with final result
/// - `Err(PluginError)` - Spawn, timeout, exit, or protocol error
///
/// # Protocol
///
/// 1. Spawn process with piped stdio
/// 2. Start background task to drain stderr
/// 3. Send `initialize` request with `id: 0` (reserved for initialization)
/// 4. Enter `tokio::select!` loop with 3 branches:
///    - **Reader**: Read stdout line → decode → if Request: dispatch and send response
///    - **Writer**: Receive response from channel → write to stdin
///    - **Timeout**: Kill process on timeout
/// 5. When response with `id: 0` received, extract result and exit
/// 6. Collect stderr and return result
pub async fn run_jsonrpc(
    manifest: &PluginManifest,
    manifest_dir: &Path,
    args: &Value,
    timeout_secs: u64,
    permissions: &[Permission],
    dispatcher: &RpcDispatcher,
    ctx: &DispatcherContext,
) -> Result<PluginResult, PluginError> {
    use crate::plugin::runner::simple::spawn_process;

    let start = std::time::Instant::now();
    let mut child = spawn_process(manifest, manifest_dir)?;

    // Start stderr collector in background
    let stderr_collector = StderrCollector::new(child.stderr.take());

    // Split stdin/stdout for concurrent access
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PluginError::Io("Failed to open stdin".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PluginError::Io("Failed to open stdout".to_string()))?;

    let mut reader_lines =
        FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_MESSAGE_SIZE));

    // Channel for response writer (buffer size 16)
    let (response_tx, mut response_rx) = mpsc::channel::<RpcResponse>(16);

    // Send initialize request (id: 0 is reserved)
    let init_request = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::Number(0.into())),
        method: "initialize".to_string(),
        params: Some(args.clone()),
    };
    let init_msg = RpcMessage::Request(init_request);
    let init_encoded = encode(&init_msg);

    stdin
        .write_all(init_encoded.as_bytes())
        .await
        .map_err(|e| PluginError::Io(format!("Failed to write initialize request: {}", e)))?;
    stdin
        .flush()
        .await
        .map_err(|e| PluginError::Io(format!("Failed to flush stdin: {}", e)))?;

    // Track final result (id: 0 response)
    #[allow(unused_assignments)]
    let mut final_result: Option<Value> = None;

    // Main event loop with timeout
    let timeout_duration = Duration::from_secs(timeout_secs);
    let sleep_future = tokio::time::sleep(timeout_duration);

    tokio::pin!(sleep_future);

    loop {
        tokio::select! {
            // Reader branch: read stdout and handle messages
            line_result = reader_lines.next() => {
                match line_result {
                    Some(Ok(line)) => {
                        let msg = decode(&line);
                        let msg = match msg {
                            Some(m) => m,
                            None => continue,
                        };

                        match msg {
                            RpcMessage::Request(request) => {
                                let response = dispatcher.dispatch(request, permissions, ctx).await;
                                if response_tx.send(response).await.is_err() {
                                    return Err(PluginError::Io("Response channel closed".to_string()));
                                }
                            }
                            RpcMessage::Response(response) => {
                                if response.id == Value::Number(0.into()) {
                                    if let Some(error) = response.error {
                                        return Err(PluginError::InvalidOutput(format!(
                                            "Initialize failed: {} (code {})",
                                            error.message, error.code
                                        )));
                                    }
                                    final_result = response.result;
                                    break;
                                }
                            }
                        }
                    }
                    None => {
                        return Err(PluginError::InvalidOutput("Unexpected EOF from script".to_string()));
                    }
                    Some(Err(e)) => {
                        return Err(PluginError::Io(format!("Failed to read stdout: {}", e)));
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
                            .map_err(|e| PluginError::Io(format!("Failed to write response: {}", e)))?;
                        stdin.flush().await
                            .map_err(|e| PluginError::Io(format!("Failed to flush stdin: {}", e)))?;
                    }
                    None => {
                        return Err(PluginError::Io("Response channel closed".to_string()));
                    }
                }
            }

            // Timeout branch
            _ = &mut sleep_future => {
                child.kill().await
                    .map_err(|e| PluginError::Io(format!("Failed to kill timed-out process: {}", e)))?;
                return Err(PluginError::Timeout(timeout_secs));
            }
        }
    }

    // Wait for child exit
    let exit_status = child
        .wait()
        .await
        .map_err(|e| PluginError::Io(format!("Failed to wait for process: {}", e)))?;

    if !exit_status.success() {
        return Err(PluginError::NonZeroExit(exit_status.code()));
    }

    // Collect stderr
    let stderr = stderr_collector.collect().await;

    // Extract final result
    let output = final_result.ok_or_else(|| {
        PluginError::InvalidOutput("No result received from initialize".to_string())
    })?;

    Ok(PluginResult {
        output,
        stderr,
        duration: start.elapsed(),
    })
}
