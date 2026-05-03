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

    /// Like `get_definitions` but optionally filters by an allowlist.
    ///
    /// `None` returns every registered tool (same as `get_definitions`).
    /// `Some(slice)` returns only those whose name appears in the slice.
    /// `Some(&[])` returns no tools at all — explicit "forbid all".
    pub fn get_definitions_filtered(&self, filter: Option<&[String]>) -> Vec<ToolDefinition> {
        self.items
            .iter()
            .filter(|(name, _)| match filter {
                None => true,
                Some(set) => set.iter().any(|s| s == *name),
            })
            .map(|(_, entry)| {
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
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;

    struct Stub(&'static str);

    #[async_trait]
    impl Tool for Stub {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({})
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
            unreachable!()
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Box::new(Stub("alpha")));
        r.register(Box::new(Stub("beta")));
        r.register(Box::new(Stub("gamma")));
        r
    }

    #[test]
    fn none_filter_returns_all() {
        let r = make_registry();
        let defs = r.get_definitions_filtered(None);
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn empty_filter_returns_no_tools() {
        let r = make_registry();
        let defs = r.get_definitions_filtered(Some(&[]));
        assert_eq!(defs.len(), 0);
    }

    #[test]
    fn whitelist_filters_to_named() {
        let r = make_registry();
        let names = vec!["alpha".to_string(), "gamma".to_string()];
        let defs = r.get_definitions_filtered(Some(&names));
        let got: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(got.contains(&"alpha"));
        assert!(got.contains(&"gamma"));
        assert!(!got.contains(&"beta"));
    }
}
