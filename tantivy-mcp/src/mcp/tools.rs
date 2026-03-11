//! Tool registry for MCP tools.

use serde_json::Value;
use std::collections::HashMap;

use super::types::{McpTool, ToolResult};
use crate::Result;

/// A tool handler function.
pub type ToolHandler = Box<dyn Fn(Option<Value>) -> Result<ToolResult> + Send + Sync>;

/// Registry of MCP tools.
pub struct ToolRegistry {
    tools: HashMap<String, (McpTool, ToolHandler)>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool with its handler.
    pub fn register<F>(&mut self, tool: McpTool, handler: F)
    where
        F: Fn(Option<Value>) -> Result<ToolResult> + Send + Sync + 'static,
    {
        self.tools
            .insert(tool.name.clone(), (tool, Box::new(handler)));
    }

    /// Get all registered tools for tools/list.
    pub fn list_tools(&self) -> Vec<McpTool> {
        self.tools.values().map(|(tool, _)| tool.clone()).collect()
    }

    /// Call a tool by name.
    pub fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<ToolResult> {
        match self.tools.get(name) {
            Some((_, handler)) => handler(arguments),
            None => Ok(ToolResult::error(format!("Unknown tool: {}", name))),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
