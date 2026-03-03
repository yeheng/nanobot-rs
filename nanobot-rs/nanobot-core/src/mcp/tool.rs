use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::manager::McpManager;
use super::types::McpTool;
use crate::tools::{Tool, ToolError};

/// A bridge that wraps an MCP tool as an `impl Tool` so it can be registered
/// in the `ToolRegistry` alongside native tools.
///
/// Each MCP server has its own lock, so tool calls to different servers
/// execute concurrently.
pub struct McpToolBridge {
    /// MCP server name
    server_name: String,
    /// Tool name on the MCP server
    tool_name: String,
    /// Tool description
    description: String,
    /// JSON Schema for input parameters
    input_schema: Value,
    /// Shared reference to the MCP manager
    manager: Arc<McpManager>,
}

impl McpToolBridge {
    /// Create a new MCP tool bridge
    pub fn new(server_name: String, tool: &McpTool, manager: Arc<McpManager>) -> Self {
        Self {
            server_name,
            tool_name: tool.name.clone(),
            description: tool
                .description
                .clone()
                .unwrap_or_else(|| format!("MCP tool: {}", tool.name)),
            input_schema: tool
                .input_schema
                .clone()
                .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            manager,
        }
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        self.manager
            .call_tool(&self.server_name, &self.tool_name, args)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))
    }
}
