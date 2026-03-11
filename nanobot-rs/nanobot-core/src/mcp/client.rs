use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tracing::{debug, info};

use super::types::{McpServerConfig, McpTool, McpTransport};
use crate::error::McpError;

/// Expand tilde (~) in path to user's home directory
fn expand_tilde(path: &str) -> String {
    if let Some(p) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}", home.join(p).display());
        }
    }
    path.to_string()
}

/// Default timeout for MCP requests in seconds
const MCP_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Pending requests awaiting responses
type PendingRequests = Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

enum Transport {
    Stdio {
        process: Child,
        stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    },
    Http {
        url: String,
        client: reqwest::Client,
    },
}

/// MCP client for communicating with a server via JSON-RPC
pub struct McpClient {
    name: String,
    transport: Option<Transport>,
    tools: Vec<McpTool>,
    request_id: Arc<AtomicU64>,
    pending: PendingRequests,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new(name: String, _config: McpServerConfig) -> Self {
        Self {
            name,
            transport: None,
            tools: Vec::new(),
            request_id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Start the MCP server and initialize
    pub async fn start(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        info!("Starting MCP server: {}", self.name);

        match config.transport {
            McpTransport::Stdio { command, args, env } => {
                self.start_stdio(command, args, env).await?;
            }
            McpTransport::Http { url } => {
                self.start_http(url).await?;
            }
        }

        self.initialize().await?;
        self.list_tools().await?;

        info!("MCP server {} ready with {} tools", self.name, self.tools.len());
        Ok(())
    }

    async fn start_stdio(
        &mut self,
        command: String,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<(), McpError> {
        let command = expand_tilde(&command);
        let mut cmd = Command::new(&command);
        let args: Vec<String> = args.iter().map(|a| expand_tilde(a)).collect();

        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(env) = env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");

        let stdin = Arc::new(tokio::sync::Mutex::new(Some(stdin)));
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

                debug!("[MCP:{}] stdout: {}", server_name, &line[..line.len().min(200)]);

                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                        let mut pending = pending.lock().await;
                        if let Some(tx) = pending.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                }
            }
        });

        self.transport = Some(Transport::Stdio { process: child, stdin });
        Ok(())
    }

    async fn start_http(&mut self, url: String) -> Result<(), McpError> {
        let client = reqwest::Client::new();
        self.transport = Some(Transport::Http { url, client });
        Ok(())
    }

    /// Send a JSON-RPC request and wait for the response
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(Value::Object(serde_json::Map::new()))
        });

        match &self.transport {
            Some(Transport::Stdio { stdin, .. }) => {
                self.send_request_stdio(id, request, stdin).await
            }
            Some(Transport::Http { url, client }) => {
                self.send_request_http(request, url, client).await
            }
            None => Err(McpError::ConnectionError("Transport not initialized".into())),
        }
    }

    async fn send_request_stdio(
        &self,
        id: u64,
        request: Value,
        stdin: &Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    ) -> Result<Value, McpError> {
        let request_str = serde_json::to_string(&request)?;
        debug!("[MCP:{}] → {}", self.name, &request_str[..request_str.len().min(200)]);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        {
            let mut stdin_lock = stdin.lock().await;
            if let Some(ref mut stdin) = *stdin_lock {
                stdin.write_all(request_str.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
            } else {
                return Err(McpError::ConnectionError(format!("MCP server {} stdin not available", self.name)));
            }
        }

        let response = tokio::time::timeout(std::time::Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| McpError::TimeoutError(format!("MCP server {} timed out", self.name)))?
            .map_err(|_| McpError::ConnectionError(format!("MCP server {} dropped response", self.name)))?;

        if let Some(err) = response.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(McpError::JsonRpcError { code, message: message.to_string() });
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn send_request_http(
        &self,
        request: Value,
        url: &str,
        client: &reqwest::Client,
    ) -> Result<Value, McpError> {
        let response = client
            .post(url)
            .json(&request)
            .timeout(std::time::Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| McpError::ConnectionError(format!("HTTP request failed: {}", e)))?;

        let response: Value = response
            .json()
            .await
            .map_err(|e| McpError::ConnectionError(format!("Failed to parse response: {}", e)))?;

        if let Some(err) = response.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(McpError::JsonRpcError { code, message: message.to_string() });
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

        match &self.transport {
            Some(Transport::Stdio { stdin, .. }) => {
                let notification_str = serde_json::to_string(&notification)?;
                let mut stdin_lock = stdin.lock().await;
                if let Some(ref mut stdin) = *stdin_lock {
                    stdin.write_all(notification_str.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await?;
                }
                Ok(())
            }
            Some(Transport::Http { .. }) => Ok(()),
            None => Err(McpError::ConnectionError("Transport not initialized".into())),
        }
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
            Ok(serde_json::to_string(&result)?)
        }
    }

    /// Stop the MCP server
    pub async fn stop(&mut self) -> Result<(), McpError> {
        if let Some(transport) = self.transport.take() {
            if let Transport::Stdio { mut process, .. } = transport {
                process.kill().await?;
                info!("MCP server {} stopped", self.name);
            }
        }
        Ok(())
    }
}
