//! Tool registry for managing and executing tools

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolMetadata, ToolResult};
use gasket_providers::ToolDefinition;
use gasket_types::{ApprovalCallback, ToolApprovalRequest};

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
    approval_callback: Option<Arc<dyn ApprovalCallback>>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            approval_callback: self.approval_callback.clone(),
        }
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            approval_callback: None,
        }
    }

    /// Set an approval callback that will be invoked before executing any tool
    /// whose metadata has `requires_approval == true`.
    pub fn with_approval_callback(mut self, callback: Arc<dyn ApprovalCallback>) -> Self {
        self.approval_callback = Some(callback);
        self
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

        // Check if approval is required
        if let Some(ref meta) = entry.metadata {
            if meta.requires_approval {
                if let Some(ref callback) = self.approval_callback {
                    let request = ToolApprovalRequest {
                        id: uuid::Uuid::new_v4().to_string(),
                        tool_name: name.to_string(),
                        description: if meta.display_name.is_empty() {
                            format!("Execute tool '{}'", name)
                        } else {
                            meta.display_name.clone()
                        },
                        arguments: args.to_string(),
                    };

                    match callback.request_approval(&ctx.session_key, request).await {
                        Ok(true) => {
                            debug!("Tool {} approved by user", name);
                        }
                        Ok(false) => {
                            return Err(ToolError::PermissionDenied(format!(
                                "User denied execution of tool '{}'",
                                name
                            )));
                        }
                        Err(e) => {
                            return Err(ToolError::PermissionDenied(format!(
                                "Approval check failed for tool '{}': {}",
                                name, e
                            )));
                        }
                    }
                }
            }
        }

        debug!("Executing tool: {} with args: {:?}", name, args);
        entry.tool.execute(args, ctx).await
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<&str> {
        self.items.keys().map(|s| s.as_str()).collect()
    }

    /// Return a new registry containing only the tools whose names appear in `allowed`.
    ///
    /// An empty `allowed` slice returns an **empty** registry (not all tools).
    /// Callers that want "all tools" should clone the original registry instead.
    pub fn filtered(&self, allowed: &[&str]) -> ToolRegistry {
        let mut filtered = ToolRegistry::new();
        for (name, entry) in &self.items {
            if allowed.contains(&name.as_str()) {
                filtered.items.insert(
                    name.clone(),
                    entry.clone(),
                );
            }
        }
        filtered
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> ToolResult {
            Ok("ok".into())
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(FakeTool { name: "wiki_search" }));
        reg.register(Box::new(FakeTool { name: "wiki_read" }));
        reg.register(Box::new(FakeTool { name: "shell" }));
        reg.register(Box::new(FakeTool { name: "write_file" }));
        reg.register(Box::new(FakeTool { name: "phase_transition" }));
        reg
    }

    #[test]
    fn test_filtered_returns_only_allowed_tools() {
        let registry = make_registry();
        let filtered = registry.filtered(&["wiki_search", "wiki_read", "phase_transition"]);
        let names = filtered.list();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"wiki_search"));
        assert!(names.contains(&"wiki_read"));
        assert!(names.contains(&"phase_transition"));
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"write_file"));
    }

    #[test]
    fn test_filtered_empty_allowed_gives_empty_registry() {
        let registry = make_registry();
        let filtered = registry.filtered(&[]);
        assert!(filtered.list().is_empty());
    }

    #[test]
    fn test_filtered_preserves_definitions() {
        let registry = make_registry();
        let filtered = registry.filtered(&["shell"]);
        let defs = filtered.get_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "shell");
    }
}
