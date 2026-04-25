//! Tool registry for managing and executing tools

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolMetadata, ToolResult};
use gasket_providers::LlmProvider;
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

    /// Set metadata for an already-registered tool
    pub fn set_metadata(&mut self, name: &str, meta: ToolMetadata) {
        if let Some(entry) = self.items.get_mut(name) {
            entry.metadata = Some(meta);
        }
    }

    /// Inject engine references into all registered plugins.
    pub fn link_engine_refs(&mut self, registry: Arc<Self>, provider: Arc<dyn LlmProvider>) {
        let resources = crate::plugin::EngineResources {
            tool_registry: registry,
            provider,
        };
        for entry in self.items.values_mut() {
            if let Some(plugin_tool) = entry
                .tool
                .as_any()
                .downcast_ref::<crate::plugin::PluginTool>()
            {
                let updated = plugin_tool.clone().with_engine_refs(resources.clone());
                entry.tool = Arc::new(updated);
            }
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
