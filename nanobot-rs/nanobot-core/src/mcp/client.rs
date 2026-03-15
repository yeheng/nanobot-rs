use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

#[cfg(feature = "mcp-websocket")]
use futures::{SinkExt, StreamExt};

use super::types::{McpAuth, McpServerConfig, McpTool, McpTransport};
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

/// Expand environment variables in a string (e.g., "${API_KEY}" -> "actual-key")
fn expand_env_vars(s: &str) -> String {
    // Check if the string matches ${...} pattern (minimum "${X}" is 4 chars)
    if s.len() >= 4 && s.starts_with("${") && s.ends_with('}') {
        // Safe slice: we verified the prefix and suffix are ASCII chars
        let var_name = &s[2..s.len() - 1];
        if let Ok(env_value) = std::env::var(var_name) {
            return env_value;
        }
    }
    s.to_string()
}

/// Apply authentication headers to a request builder
fn apply_auth(mut req: reqwest::RequestBuilder, auth: &McpAuth) -> reqwest::RequestBuilder {
    if let Some(api_key) = &auth.api_key {
        let api_key = expand_env_vars(api_key);
        req = req.header("X-API-Key", api_key);
    }
    if let Some(bearer) = &auth.bearer_token {
        let bearer = expand_env_vars(bearer);
        req = req.bearer_auth(bearer);
    }
    if let Some(headers) = &auth.headers {
        for (key, value) in headers {
            let value = expand_env_vars(value);
            req = req.header(key, value);
        }
    }
    req
}

/// Default timeout for MCP requests in seconds
const MCP_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Pending requests awaiting responses
type PendingRequests = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

enum Transport {
    Stdio {
        process: Child,
        stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    },
    Http {
        url: String,
        client: reqwest::Client,
        auth: McpAuth,
        timeout: u64,
    },
    Sse {
        url: String,
        client: reqwest::Client,
        auth: McpAuth,
        timeout: u64,
        // Channel to receive responses from SSE event stream (wrapped in Mutex for interior mutability)
        event_rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Value>>>,
        // Shutdown signal sender
        shutdown_tx: tokio::sync::oneshot::Sender<()>,
    },
    #[cfg(feature = "mcp-websocket")]
    WebSocket {
        timeout: u64,
        // Channel to send WebSocket messages
        request_tx: tokio::sync::mpsc::Sender<String>,
        // Channel to receive responses from WebSocket read task (wrapped in Mutex for interior mutability)
        response_rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Value>>>,
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
            pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Start the MCP server and initialize
    pub async fn start(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        info!("Starting MCP server: {}", self.name);

        match config.transport {
            McpTransport::Stdio { command, args, env } => {
                self.start_stdio(command, args, env).await?;
            }
            McpTransport::Http { url, auth, timeout } => {
                self.start_http(url, auth, timeout).await?;
            }
            McpTransport::Sse { url, auth, timeout } => {
                self.start_sse(url, auth, timeout).await?;
            }
            #[cfg(feature = "mcp-websocket")]
            McpTransport::WebSocket { url, auth, timeout } => {
                self.start_websocket(url, auth, timeout).await?;
            }
            #[cfg(not(feature = "mcp-websocket"))]
            McpTransport::WebSocket { .. } => {
                return Err(McpError::WebSocketError(
                    "WebSocket transport requires 'mcp-websocket' feature".into(),
                ));
            }
        }

        self.initialize().await?;
        self.list_tools().await?;

        info!(
            "MCP server {} ready with {} tools",
            self.name,
            self.tools.len()
        );
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

                debug!("[MCP:{}] stdout: {}", server_name, &line);

                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                        let mut pending = pending.lock().unwrap();
                        if let Some(tx) = pending.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                }
            }
        });

        self.transport = Some(Transport::Stdio {
            process: child,
            stdin,
        });
        Ok(())
    }

    async fn start_http(
        &mut self,
        url: String,
        auth: McpAuth,
        timeout: u64,
    ) -> Result<(), McpError> {
        let client = reqwest::Client::new();
        self.transport = Some(Transport::Http {
            url,
            client,
            auth,
            timeout,
        });
        Ok(())
    }

    /// Start SSE transport (Server-Sent Events)
    async fn start_sse(
        &mut self,
        url: String,
        auth: McpAuth,
        timeout: u64,
    ) -> Result<(), McpError> {
        let client = reqwest::Client::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Spawn background task to receive SSE events
        let sse_url = format!("{}/sse", url.trim_end_matches('/'));
        let server_name = self.name.clone();

        // Clone values for the async task
        let client_clone = client.clone();
        let auth_clone = auth.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::sse_event_loop(
                &client_clone,
                &sse_url,
                &auth_clone,
                event_tx,
                shutdown_rx,
                &server_name,
            )
            .await
            {
                warn!("[MCP:{}] SSE event loop error: {}", server_name, e);
            }
        });

        self.transport = Some(Transport::Sse {
            url,
            client,
            auth,
            timeout,
            event_rx: Arc::new(tokio::sync::Mutex::new(event_rx)),
            shutdown_tx,
        });
        Ok(())
    }

    /// SSE event loop for receiving responses
    async fn sse_event_loop(
        client: &reqwest::Client,
        sse_url: &str,
        auth: &McpAuth,
        event_tx: tokio::sync::mpsc::Sender<Value>,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
        server_name: &str,
    ) -> Result<(), McpError> {
        let mut request = client.get(sse_url);
        request = apply_auth(request, auth);

        let response = request
            .send()
            .await
            .map_err(|e| McpError::ConnectionError(format!("SSE connection failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(McpError::ConnectionError(format!(
                "SSE connection failed with status: {}",
                response.status()
            )));
        }

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("[MCP:{}] SSE event loop shutting down", server_name);
                    break;
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));

                            // Parse SSE events from buffer
                            while let Some(pos) = buffer.find("\n\n") {
                                let event_data = buffer[..pos].to_string();
                                buffer = buffer[pos + 2..].to_string();

                                // Parse the event
                                if let Some(json) = Self::parse_sse_event(&event_data) {
                                    debug!("[MCP:{}] SSE event: {}", server_name, &json.to_string());
                                    let _ = event_tx.send(json).await;
                                }
                            }
                        }
                        Some(Err(e)) => {
                            warn!("[MCP:{}] SSE stream error: {}", server_name, e);
                            break;
                        }
                        None => {
                            debug!("[MCP:{}] SSE stream ended", server_name);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse an SSE event and extract JSON data
    fn parse_sse_event(event_data: &str) -> Option<Value> {
        for line in event_data.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(json) = serde_json::from_str::<Value>(data) {
                    return Some(json);
                }
            }
        }
        None
    }

    /// Start WebSocket transport
    #[cfg(feature = "mcp-websocket")]
    async fn start_websocket(
        &mut self,
        url: String,
        _auth: McpAuth,
        timeout: u64,
    ) -> Result<(), McpError> {
        use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

        // Build WebSocket URL
        let ws_url = url
            .replace("http://", "ws://")
            .replace("https://", "wss://");

        let (ws_stream, _) = connect_async(&ws_url).await.map_err(|e| {
            McpError::ConnectionError(format!("WebSocket connection failed: {}", e))
        })?;

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<String>(64);
        let (response_tx, response_rx) = tokio::sync::mpsc::channel(64);
        let server_name = self.name.clone();

        // Spawn background task to handle WebSocket I/O
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle outgoing messages
                    Some(msg) = request_rx.recv() => {
                        if let Err(e) = ws_sink.send(Message::Text(msg.into())).await {
                            warn!("[MCP:{}] WebSocket send error: {}", server_name, e);
                            break;
                        }
                    }
                    // Handle incoming messages
                    msg = ws_stream.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(json) = serde_json::from_str::<Value>(&text) {
                                    debug!(
                                        "[MCP:{}] WS message: {}",
                                        server_name,
                                        &text
                                    );
                                    let _ = response_tx.send(json).await;
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                // Respond with pong - need to get sink from somewhere
                                // For simplicity, we'll ignore pings for now (tungstenite handles this automatically)
                                let _ = data;
                            }
                            Some(Ok(Message::Close(_))) => {
                                debug!("[MCP:{}] WebSocket closed by server", server_name);
                                break;
                            }
                            Some(Err(e)) => {
                                warn!("[MCP:{}] WebSocket error: {}", server_name, e);
                                break;
                            }
                            None => {
                                debug!("[MCP:{}] WebSocket stream ended", server_name);
                                break;
                            }
                            _ => {}
                        }
                    }
                    else => break,
                }
            }
        });

        self.transport = Some(Transport::WebSocket {
            timeout,
            request_tx,
            response_rx: Arc::new(tokio::sync::Mutex::new(response_rx)),
        });
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
            Some(Transport::Http {
                url,
                client,
                auth,
                timeout,
            }) => {
                self.send_request_http(request, url, client, auth, *timeout)
                    .await
            }
            Some(Transport::Sse {
                url,
                client,
                auth,
                timeout,
                event_rx,
                ..
            }) => {
                self.send_request_sse(request, url, client, auth, *timeout, event_rx)
                    .await
            }
            #[cfg(feature = "mcp-websocket")]
            Some(Transport::WebSocket {
                request_tx,
                response_rx,
                timeout,
                ..
            }) => {
                self.send_request_websocket(id, request, request_tx, response_rx, *timeout)
                    .await
            }
            None => Err(McpError::ConnectionError(
                "Transport not initialized".into(),
            )),
        }
    }

    async fn send_request_stdio(
        &self,
        id: u64,
        request: Value,
        stdin: &Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    ) -> Result<Value, McpError> {
        let request_str = serde_json::to_string(&request)?;
        debug!("[MCP:{}] → {}", self.name, &request_str);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        {
            let mut stdin_lock = stdin.lock().await;
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

        let response =
            tokio::time::timeout(std::time::Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS), rx)
                .await
                .map_err(|_| McpError::TimeoutError(format!("MCP server {} timed out", self.name)))?
                .map_err(|_| {
                    McpError::ConnectionError(format!("MCP server {} dropped response", self.name))
                })?;

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

    async fn send_request_http(
        &self,
        request: Value,
        url: &str,
        client: &reqwest::Client,
        auth: &McpAuth,
        timeout: u64,
    ) -> Result<Value, McpError> {
        let mut req = client
            .post(url)
            .json(&request)
            .timeout(std::time::Duration::from_secs(timeout));
        req = apply_auth(req, auth);

        let response = req
            .send()
            .await
            .map_err(|e| McpError::ConnectionError(format!("HTTP request failed: {}", e)))?;

        let response: Value = response
            .json()
            .await
            .map_err(|e| McpError::ConnectionError(format!("Failed to parse response: {}", e)))?;

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

    async fn send_request_sse(
        &self,
        request: Value,
        url: &str,
        client: &reqwest::Client,
        auth: &McpAuth,
        timeout: u64,
        event_rx: &Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Value>>>,
    ) -> Result<Value, McpError> {
        let id = request.get("id").and_then(|v| v.as_u64()).unwrap_or(0);

        // POST the request to the message endpoint
        let message_url = format!("{}/message", url.trim_end_matches('/'));
        let mut req = client
            .post(&message_url)
            .json(&request)
            .timeout(std::time::Duration::from_secs(timeout));
        req = apply_auth(req, auth);

        req.send()
            .await
            .map_err(|e| McpError::ConnectionError(format!("SSE POST failed: {}", e)))?;

        // Wait for the response from the SSE event stream
        let response = tokio::time::timeout(std::time::Duration::from_secs(timeout), async {
            let mut rx = event_rx.lock().await;
            while let Some(event) = rx.recv().await {
                if event.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return Ok(event);
                }
            }
            Err(McpError::ConnectionError("SSE stream closed".into()))
        })
        .await
        .map_err(|_| {
            McpError::TimeoutError(format!("SSE response timed out for request {}", id))
        })??;

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

    #[cfg(feature = "mcp-websocket")]
    async fn send_request_websocket(
        &self,
        id: u64,
        request: Value,
        request_tx: &tokio::sync::mpsc::Sender<String>,
        response_rx: &Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Value>>>,
        timeout: u64,
    ) -> Result<Value, McpError> {
        let request_str = serde_json::to_string(&request)?;
        debug!("[MCP:{}] WS → {}", self.name, &request_str);

        // Send the request through the channel
        request_tx
            .send(request_str)
            .await
            .map_err(|e| McpError::ConnectionError(format!("WebSocket send failed: {}", e)))?;

        // Wait for the response
        let response = tokio::time::timeout(std::time::Duration::from_secs(timeout), async {
            let mut rx = response_rx.lock().await;
            while let Some(msg) = rx.recv().await {
                if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return Ok(msg);
                }
            }
            Err(McpError::ConnectionError("WebSocket stream closed".into()))
        })
        .await
        .map_err(|_| {
            McpError::TimeoutError(format!("WebSocket response timed out for request {}", id))
        })??;

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
            Some(Transport::Sse { .. }) => Ok(()),
            #[cfg(feature = "mcp-websocket")]
            Some(Transport::WebSocket { request_tx, .. }) => {
                let notification_str = serde_json::to_string(&notification)?;
                request_tx.send(notification_str).await.map_err(|e| {
                    McpError::ConnectionError(format!("WebSocket send failed: {}", e))
                })?;
                Ok(())
            }
            None => Err(McpError::ConnectionError(
                "Transport not initialized".into(),
            )),
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
        match self.transport.take() {
            Some(Transport::Stdio { mut process, .. }) => {
                process.kill().await?;
                info!("MCP server {} stopped", self.name);
            }
            Some(Transport::Sse { shutdown_tx, .. }) => {
                // Signal the SSE event loop to stop
                let _ = shutdown_tx.send(());
                info!("MCP server {} SSE connection closed", self.name);
            }
            #[cfg(feature = "mcp-websocket")]
            Some(Transport::WebSocket { .. }) => {
                // Dropping the transport will close the request channel,
                // which will cause the background task to exit
                info!("MCP server {} WebSocket connection closed", self.name);
            }
            Some(Transport::Http { .. }) => {
                // HTTP is stateless, nothing to clean up
                info!("MCP server {} HTTP client released", self.name);
            }
            None => {}
        }
        Ok(())
    }

    /// Send a ping to check server health (for remote transports)
    pub async fn ping(&self) -> Result<(), McpError> {
        // Use the 'ping' method which is part of MCP spec
        let _ = self.send_request("ping", None).await?;
        Ok(())
    }
}
