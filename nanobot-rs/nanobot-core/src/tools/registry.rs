//! Tool registry for managing and executing tools

use std::collections::HashMap;

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolError, ToolMetadata, ToolResult};
use crate::providers::ToolDefinition;

/// Registry for managing tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    metadata: HashMap<String, ToolMetadata>,
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        debug!("Registering tool: {}", name);
        self.tools.insert(name, tool);
    }

    /// Register a tool with associated metadata
    pub fn register_with_metadata(&mut self, tool: Box<dyn Tool>, meta: ToolMetadata) {
        let name = tool.name().to_string();
        debug!(
            "Registering tool with metadata: {} (category: {:?})",
            name, meta.category
        );
        self.metadata.insert(name.clone(), meta);
        self.tools.insert(name, tool);
    }

    /// Set metadata for an already-registered tool
    pub fn set_metadata(&mut self, name: &str, meta: ToolMetadata) {
        self.metadata.insert(name.to_string(), meta);
    }

    /// Get metadata for a tool
    pub fn get_metadata(&self, name: &str) -> Option<&ToolMetadata> {
        self.metadata.get(name)
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Get all tool definitions for LLM
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| {
                ToolDefinition::function(tool.name(), tool.description(), tool.parameters())
            })
            .collect()
    }

    /// Execute a tool by name
    #[instrument(skip(self, args))]
    pub async fn execute(&self, name: &str, args: Value) -> ToolResult {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(format!("Tool not found: {}", name)))?;

        debug!("Executing tool: {} with args: {:?}", name, args);
        tool.execute(args).await
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// List tools by category (from metadata)
    pub fn list_by_category(&self, category: &str) -> Vec<&str> {
        self.metadata
            .iter()
            .filter(|(_, m)| m.category == category)
            .filter_map(|(name, _)| {
                if self.tools.contains_key(name) {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    /// List tools that require approval (from metadata)
    pub fn list_requiring_approval(&self) -> Vec<&str> {
        self.metadata
            .iter()
            .filter(|(_, m)| m.requires_approval)
            .filter_map(|(name, _)| {
                if self.tools.contains_key(name) {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    /// List tools that are mutating (from metadata)
    pub fn list_mutating(&self) -> Vec<&str> {
        self.metadata
            .iter()
            .filter(|(_, m)| m.is_mutating)
            .filter_map(|(name, _)| {
                if self.tools.contains_key(name) {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
