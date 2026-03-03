use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tracing::{debug, info};

use super::types::{McpServerConfig, McpTool};
use crate::error::McpError;

/// Pending requests awaiting responses
type PendingRequests = Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// MCP client for communicating with a server via JSON-RPC over stdio
pub struct McpClient {
    name: String,
    config: McpServerConfig,
    process: Option<Child>,
    /// Stdin wrapped in Arc<Mutex> for concurrent write access
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    tools: Vec<McpTool>,
    /// Atomic request ID counter for lock-free ID generation
    request_id: Arc<AtomicU64>,
    pending: PendingRequests,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new(name: String, config: McpServerConfig) -> Self {
        Self {
            name,
            config,
            process: None,
            stdin: Arc::new(tokio::sync::Mutex::new(None)),
            tools: Vec::new(),
            request_id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Start the MCP server process and initialize
    pub async fn start(&mut self) -> Result<(), McpError> {
        info!("Starting MCP server: {}", self.name);

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(env) = &self.config.env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }

        let mut child = cmd.spawn()?;

        let stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");

        // Store stdin in Arc<Mutex> for concurrent access
        *self.stdin.lock().await = Some(stdin);

        // Spawn a task to read stdout and dispatch responses
        let pending = self.pending.clone();
        let server_name = self.name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                debug!(
                    "[MCP:{}] stdout: {}",
                    server_name,
                    &line[..line.len().min(200)]
                );

                match serde_json::from_str::<Value>(&line) {
                    Ok(msg) => {
                        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                            // This is a response — find the pending request
                            let mut pending = pending.lock().await;
                            if let Some(tx) = pending.remove(&id) {
                                let _ = tx.send(msg);
                            }
                        }
                        // Notifications (no id) are logged but not dispatched
                    }
                    Err(e) => {
                        debug!(
                            "[MCP:{}] non-JSON line: {} ({})",
                            server_name,
                            &line[..line.len().min(80)],
                            e
                        );
                    }
                }
            }

            debug!("[MCP:{}] stdout reader exited", server_name);
        });

        self.process = Some(child);

        // Initialize connection
        self.initialize().await?;

        // List available tools
        self.list_tools().await?;

        info!(
            "MCP server {} ready with {} tools",
            self.name,
            self.tools.len()
        );
        Ok(())
    }

    /// Send a JSON-RPC request and wait for the response
    ///
    /// This method only holds the stdin lock during the write operation,
    /// allowing concurrent requests to be multiplexed over the same connection.
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        // Generate ID atomically (lock-free)
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(Value::Object(serde_json::Map::new()))
        });

        let request_str = serde_json::to_string(&request)?;
        debug!(
            "[MCP:{}] → {}",
            self.name,
            &request_str[..request_str.len().min(200)]
        );

        // Register pending request BEFORE sending
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // Write to stdin - only hold lock during write operation
        {
            let mut stdin_lock = self.stdin.lock().await;
            if let Some(ref mut stdin) = *stdin_lock {
                stdin.write_all(request_str.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
            } else {
                return Err(McpError::ConnectionError(format!(
                    "MCP server {} stdin not available",
                    self.name
                )));
            }
        }
        // Lock released here - response will arrive asynchronously

        // Wait for response with timeout (no lock held during wait!)
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                McpError::TimeoutError(format!(
                    "MCP server {} timed out on method '{}'",
                    self.name, method
                ))
            })?
            .map_err(|_| {
                McpError::ConnectionError(format!(
                    "MCP server {} dropped response channel",
                    self.name
                ))
            })?;

        // Check for JSON-RPC error
        if let Some(err) = response.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Err(McpError::JsonRpcError {
                code,
                message: message.to_string(),
            });
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send a JSON-RPC notification (no response expected)
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Object(serde_json::Map::new()))
        });

        let notification_str = serde_json::to_string(&notification)?;
        debug!("[MCP:{}] notification → {}", self.name, method);

        let mut stdin_lock = self.stdin.lock().await;
        if let Some(ref mut stdin) = *stdin_lock {
            stdin.write_all(notification_str.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        Ok(())
    }

    async fn initialize(&mut self) -> Result<(), McpError> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "nanobot",
                "version": "2.0.0"
            }
        });

        let result = self.send_request("initialize", Some(params)).await?;
        debug!("[MCP:{}] initialized: {:?}", self.name, result);

        // Send initialized notification
        self.send_notification("notifications/initialized", None)
            .await?;

        info!("MCP server {} initialized", self.name);
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<(), McpError> {
        let result = self.send_request("tools/list", None).await?;

        // Parse tools from response
        if let Some(tools_array) = result.get("tools").and_then(|v| v.as_array()) {
            self.tools = tools_array
                .iter()
                .filter_map(|t| serde_json::from_value(t.clone()).ok())
                .collect();

            info!("[MCP:{}] discovered {} tools", self.name, self.tools.len());
            for tool in &self.tools {
                debug!(
                    "[MCP:{}] tool: {} — {}",
                    self.name,
                    tool.name,
                    tool.description.as_deref().unwrap_or("(no description)")
                );
            }
        }

        Ok(())
    }

    /// Get available tools from this server
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Call a tool on this server
    ///
    /// This method is fully concurrent-safe and can be called from multiple
    /// tasks simultaneously. Only the stdin write is locked, not the wait.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, McpError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });

        let result = self.send_request("tools/call", Some(params)).await?;

        // Extract text content from the result
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            let text: Vec<&str> = content
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        item.get("text").and_then(|v| v.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            Ok(text.join("\n"))
        } else {
            Ok(serde_json::to_string_pretty(&result)?)
        }
    }

    /// Stop the MCP server
    pub async fn stop(&mut self) -> Result<(), McpError> {
        if let Some(ref mut process) = self.process {
            process.kill().await?;
            info!("MCP server {} stopped", self.name);
        }
        self.process = None;
        *self.stdin.lock().await = None;
        Ok(())
    }
}
