//! Tool registry for managing and executing tools

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolMetadata, ToolResult};
use gasket_providers::ToolDefinition;

/// A tool bundled with its optional metadata.
struct RegisteredTool {
    tool: Arc<dyn Tool>,
    metadata: Option<ToolMetadata>,
}

impl Clone for RegisteredTool {
    fn clone(&self) -> Self {
        let tool = if let Some(cloned) = self.tool.clone_box() {
            Arc::from(cloned)
        } else {
            self.tool.clone()
        };
        Self {
            tool,
            metadata: self.metadata.clone(),
        }
    }
}

/// Registry for managing tools.
pub struct ToolRegistry {
    items: HashMap<String, RegisteredTool>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
        }
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        let tool = Arc::from(tool);
        debug!("Registering tool: {}", name);
        self.items.insert(
            name,
            RegisteredTool {
                tool,
                metadata: None,
            },
        );
    }

    /// Register a tool with associated metadata
    pub fn register_with_metadata(&mut self, tool: Box<dyn Tool>, meta: ToolMetadata) {
        let name = tool.name().to_string();
        let tool = Arc::from(tool);
        debug!(
            "Registering tool with metadata: {} (category: {:?})",
            name, meta.category
        );
        self.items.insert(
            name,
            RegisteredTool {
                tool,
                metadata: Some(meta),
            },
        );
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.items.get(name).map(|e| e.tool.as_ref())
    }

    /// Get all tool definitions for LLM
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.items
            .values()
            .map(|entry| {
                ToolDefinition::function(
                    entry.tool.name(),
                    entry.tool.description(),
                    entry.tool.parameters(),
                )
            })
            .collect()
    }

    /// Execute a tool by name with context
    #[instrument(skip(self, args, ctx))]
    pub async fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> ToolResult {
        let entry = self
            .items
            .get(name)
            .ok_or_else(|| ToolError::NotFound(format!("Tool not found: {}", name)))?;

        debug!("Executing tool: {} with args: {:?}", name, args);
        entry.tool.execute(args, ctx).await
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<&str> {
        self.items.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
