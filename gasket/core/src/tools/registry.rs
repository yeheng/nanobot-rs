//! Tool registry for managing and executing tools

use std::collections::HashMap;

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolError, ToolMetadata, ToolResult};
use crate::providers::ToolDefinition;

/// A tool bundled with its optional metadata.
struct RegisteredTool {
    tool: Box<dyn Tool>,
    metadata: Option<ToolMetadata>,
}

/// Registry for managing tools
pub struct ToolRegistry {
    items: HashMap<String, RegisteredTool>,
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

    /// Set metadata for an already-registered tool
    pub fn set_metadata(&mut self, name: &str, meta: ToolMetadata) {
        if let Some(entry) = self.items.get_mut(name) {
            entry.metadata = Some(meta);
        }
    }

    /// Get metadata for a tool
    pub fn get_metadata(&self, name: &str) -> Option<&ToolMetadata> {
        self.items.get(name).and_then(|e| e.metadata.as_ref())
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

    /// Execute a tool by name
    #[instrument(skip(self, args))]
    pub async fn execute(&self, name: &str, args: Value) -> ToolResult {
        let entry = self
            .items
            .get(name)
            .ok_or_else(|| ToolError::NotFound(format!("Tool not found: {}", name)))?;

        debug!("Executing tool: {} with args: {:?}", name, args);
        entry.tool.execute(args).await
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<&str> {
        self.items.keys().map(|s| s.as_str()).collect()
    }

    /// List tools by category (from metadata)
    pub fn list_by_category(&self, category: &str) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.category == category))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// List tools that require approval (from metadata)
    pub fn list_requiring_approval(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.requires_approval))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// List tools that are mutating (from metadata)
    pub fn list_mutating(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.is_mutating))
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
