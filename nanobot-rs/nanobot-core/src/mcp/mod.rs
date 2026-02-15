//! MCP (Model Context Protocol) client implementation

use std::collections::HashMap;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tracing::{debug, info, warn};

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
}

/// MCP server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
}

/// MCP client for communicating with a server
pub struct McpClient {
    name: String,
    config: McpServerConfig,
    process: Option<Child>,
    stdin: Option<ChildStdin>,
    tools: Vec<McpTool>,
    request_id: u64,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new(name: String, config: McpServerConfig) -> Self {
        Self {
            name,
            config,
            process: None,
            stdin: None,
            tools: Vec::new(),
            request_id: 0,
        }
    }

    /// Start the MCP server process
    pub async fn start(&mut self) -> anyhow::Result<()> {
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
        let _stdout = child.stdout.take().expect("Failed to open stdout");

        self.stdin = Some(stdin);
        self.process = Some(child);

        // Initialize connection
        self.initialize().await?;

        // List available tools
        self.list_tools().await?;

        Ok(())
    }

    async fn send_request(&mut self, method: &str, params: Option<Value>) -> anyhow::Result<Value> {
        let id = self.request_id;
        self.request_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(Value::Null)
        });

        let request_str = serde_json::to_string(&request)?;
        debug!("MCP request: {}", request_str);

        if let Some(ref mut stdin) = self.stdin {
            stdin.write_all(request_str.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        // Read response (simplified - in production, use proper async read)
        Ok(Value::Null)
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "nanobot",
                "version": "2.0.0"
            }
        });

        self.send_request("initialize", Some(params)).await?;
        self.send_request("notifications/initialized", None).await?;

        info!("MCP server {} initialized", self.name);
        Ok(())
    }

    async fn list_tools(&mut self) -> anyhow::Result<()> {
        // In production, this would parse the response
        // For now, just log that we're ready
        debug!("Requesting tool list from MCP server {}", self.name);
        Ok(())
    }

    /// Get available tools from this server
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Call a tool
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });

        self.send_request("tools/call", Some(params)).await
    }

    /// Stop the MCP server
    pub async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(ref mut process) = self.process {
            process.kill().await?;
            info!("MCP server {} stopped", self.name);
        }
        self.process = None;
        self.stdin = None;
        Ok(())
    }
}

/// MCP manager for multiple servers
pub struct McpManager {
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    /// Create a new MCP manager
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Add a server
    pub fn add_server(&mut self, name: String, config: McpServerConfig) {
        let client = McpClient::new(name.clone(), config);
        self.clients.insert(name, client);
    }

    /// Start all servers
    pub async fn start_all(&mut self) -> anyhow::Result<()> {
        for (name, client) in &mut self.clients {
            if let Err(e) = client.start().await {
                warn!("Failed to start MCP server {}: {}", name, e);
            }
        }
        Ok(())
    }

    /// Get all available tools
    pub fn get_all_tools(&self) -> Vec<(&str, &McpTool)> {
        let mut tools = Vec::new();
        for (server_name, client) in &self.clients {
            for tool in client.tools() {
                tools.push((server_name.as_str(), tool));
            }
        }
        tools
    }

    /// Call a tool on a specific server
    pub async fn call_tool(
        &mut self,
        server: &str,
        name: &str,
        arguments: Value,
    ) -> anyhow::Result<Value> {
        if let Some(client) = self.clients.get_mut(server) {
            client.call_tool(name, arguments).await
        } else {
            anyhow::bail!("MCP server {} not found", server);
        }
    }

    /// Stop all servers
    pub async fn stop_all(&mut self) -> anyhow::Result<()> {
        for client in self.clients.values_mut() {
            let _ = client.stop().await;
        }
        Ok(())
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
