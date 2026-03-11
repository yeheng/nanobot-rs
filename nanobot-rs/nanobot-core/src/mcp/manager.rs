use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

use super::client::McpClient;
use super::types::{McpServerConfig, McpTool};
use crate::error::McpError;

/// MCP manager for multiple servers
pub struct McpManager {
    clients: HashMap<String, McpClient>,
    configs: HashMap<String, McpServerConfig>,
}

impl McpManager {
    /// Create a new MCP manager
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            configs: HashMap::new(),
        }
    }

    /// Add a server
    pub fn add_server(&mut self, name: String, config: McpServerConfig) {
        let client = McpClient::new(name.clone(), config.clone());
        self.configs.insert(name.clone(), config);
        self.clients.insert(name, client);
    }

    /// Start all servers
    pub async fn start_all(&mut self) -> Result<(), McpError> {
        let configs: Vec<_> = self.configs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, config) in configs {
            if let Some(client) = self.clients.get_mut(&name) {
                if let Err(e) = client.start(config).await {
                    warn!("Failed to start MCP server {}: {}", name, e);
                }
            }
        }
        Ok(())
    }

    /// Get all available tools across all servers.
    /// Returns `(server_name, tool)` pairs.
    pub async fn get_all_tools(&self) -> Vec<(String, McpTool)> {
        let mut tools = Vec::new();
        for (server_name, client) in &self.clients {
            for tool in client.tools() {
                tools.push((server_name.clone(), tool.clone()));
            }
        }
        tools
    }

    /// Call a tool on a specific server.
    ///
    /// This method is fully concurrent - multiple tool calls to the same server
    /// will be multiplexed over the same connection without blocking each other.
    pub async fn call_tool(
        &self,
        server: &str,
        name: &str,
        arguments: Value,
    ) -> Result<String, McpError> {
        if let Some(client) = self.clients.get(server) {
            // No lock needed! McpClient is fully concurrent-safe
            client.call_tool(name, arguments).await
        } else {
            Err(McpError::ServerNotFound(server.to_string()))
        }
    }

    /// Stop all servers
    pub async fn stop_all(&mut self) -> Result<(), McpError> {
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
