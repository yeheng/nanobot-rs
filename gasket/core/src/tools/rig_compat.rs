//! Rig Tool compatibility layer
//!
//! This module provides adapters to make gasket tools compatible with rig's
//! tool system. The key insight is that rig's `ToolDyn` trait is dyn-safe
//! and has a similar interface to gasket's `Tool` trait.
//!
//! # Implementation
//!
//! We create a wrapper struct `GasketToolAsRig` that wraps any gasket tool
//! and implements rig's `ToolDyn` trait, allowing gasket tools to be used
//! with rig agents.

use std::sync::Arc;

use rig::{
    completion::ToolDefinition,
    tool::ToolError,
    wasm_compat::WasmBoxedFuture,
};
use serde_json::Value;

use super::{Tool, ToolResult};

/// Wrapper that adapts a gasket Tool to implement rig's ToolDyn trait
///
/// This allows any gasket tool to be used with rig's agent system.
pub struct GasketToolAsRig<T: Tool>(pub T);

impl<T: Tool> GasketToolAsRig<T> {
    /// Create a new wrapper
    pub fn new(tool: T) -> Self {
        Self(tool)
    }
}

impl<T: Tool + Send + Sync> rig::tool::ToolDyn for GasketToolAsRig<T> {
    fn name(&self) -> String {
        self.0.name().to_string()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.0.name().to_string(),
                description: self.0.description().to_string(),
                parameters: self.0.parameters(),
            }
        })
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            // Parse args from JSON string
            let args_value: Value = serde_json::from_str(&args)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to parse tool args as JSON: {}, using raw string", e);
                    Value::String(args)
                });

            // Execute the tool
            let result: ToolResult = self.0.execute(args_value).await;

            // Convert result
            result.map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

/// Wrapper for Arc<dyn Tool> that implements rig's ToolDyn trait
///
/// This is useful for shared ownership scenarios.
pub struct ArcToolWrapper(pub Arc<dyn Tool>);

impl ArcToolWrapper {
    /// Create a new wrapper
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self(tool)
    }
}

impl rig::tool::ToolDyn for ArcToolWrapper {
    fn name(&self) -> String {
        self.0.name().to_string()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.0.name().to_string(),
                description: self.0.description().to_string(),
                parameters: self.0.parameters(),
            }
        })
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let args_value: Value = serde_json::from_str(&args)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to parse tool args as JSON: {}", e);
                    Value::String(args)
                });

            self.0.execute(args_value).await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

/// A wrapper that can hold either a boxed gasket Tool or a boxed rig ToolDyn
///
/// This is useful when you need to store heterogeneous tools in a collection.
pub enum ToolWrapper {
    /// A gasket tool
    Gasket(Box<dyn Tool>),
    /// A rig tool (dyn ToolDyn)
    Rig(Box<dyn rig::tool::ToolDyn>),
    /// An Arc-wrapped tool (for shared ownership)
    Arc(Arc<dyn Tool>),
}

impl ToolWrapper {
    /// Create a wrapper from a boxed gasket tool
    pub fn from_gasket(tool: Box<dyn Tool>) -> Self {
        Self::Gasket(tool)
    }

    /// Create a wrapper from an Arc-wrapped tool
    pub fn from_arc(tool: Arc<dyn Tool>) -> Self {
        Self::Arc(tool)
    }

    /// Get the tool name
    pub fn name(&self) -> String {
        match self {
            Self::Gasket(t) => t.name().to_string(),
            Self::Rig(t) => t.name(),
            Self::Arc(t) => t.name().to_string(),
        }
    }

    /// Get the tool definition
    pub async fn definition(&self) -> ToolDefinition {
        match self {
            Self::Gasket(t) => ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            },
            Self::Rig(t) => t.definition(String::new()).await,
            Self::Arc(t) => ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            },
        }
    }

    /// Execute the tool
    pub async fn call(&self, args: Value) -> Result<String, ToolError> {
        match self {
            Self::Gasket(t) => {
                t.execute(args).await.map_err(|e| ToolError::ToolCallError(Box::new(e)))
            }
            Self::Rig(t) => {
                let args_str = serde_json::to_string(&args).unwrap_or_default();
                t.call(args_str).await
            }
            Self::Arc(t) => {
                t.execute(args).await.map_err(|e| ToolError::ToolCallError(Box::new(e)))
            }
        }
    }
}

/// Convert a gasket ToolRegistry to a rig ToolSet
///
/// This allows using gasket tools with rig agents.
///
/// Note: This function requires the registry to provide owned tools.
/// For shared ownership, use individual `ArcToolWrapper` instances.
pub fn to_toolset_from_tools(tools: Vec<Arc<dyn Tool>>) -> rig::tool::ToolSet {
    let mut builder = rig::tool::ToolSetBuilder::default();

    for tool in tools {
        let wrapper = ArcToolWrapper(tool);
        builder = builder.static_tool(wrapper);
    }

    builder.build()
}

/// Create a rig ToolSet from tool definitions only (without execution capability)
///
/// This is useful when you only need the tool definitions for an LLM request
/// but want to handle execution separately.
pub fn definitions_to_toolset(definitions: Vec<ToolDefinition>) -> rig::tool::ToolSet {
    let mut builder = rig::tool::ToolSetBuilder::default();

    for def in definitions {
        // Create a no-op tool that just returns the definition
        let noop = NoopTool(def);
        builder = builder.static_tool(noop);
    }

    builder.build()
}

/// A no-op tool that only provides definition (for ToolSet construction)
struct NoopTool(ToolDefinition);

impl rig::tool::ToolDyn for NoopTool {
    fn name(&self) -> String {
        self.0.name.clone()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move { self.0.clone() })
    }

    fn call<'a>(&'a self, _args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            Err(ToolError::ToolCallError(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "NoopTool does not support execution",
            ))))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rig::tool::ToolDyn;
    use serde_json::json;

    /// Test tool for testing
    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "A test tool"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Test input"
                    }
                }
            })
        }

        async fn execute(&self, args: Value) -> ToolResult {
            let input = args["input"].as_str().unwrap_or("default");
            Ok(format!("Processed: {}", input))
        }
    }

    #[test]
    fn test_gasket_tool_as_rig_name() {
        let wrapper = GasketToolAsRig::new(TestTool);
        assert_eq!(wrapper.name(), "test_tool");
    }

    #[tokio::test]
    async fn test_gasket_tool_as_rig_definition() {
        let wrapper = GasketToolAsRig::new(TestTool);
        let def = wrapper.definition(String::new()).await;

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
    }

    #[tokio::test]
    async fn test_gasket_tool_as_rig_call() {
        let wrapper = GasketToolAsRig::new(TestTool);
        let args = json!({"input": "hello"});
        let args_str = serde_json::to_string(&args).unwrap();

        let result = wrapper.call(args_str).await.unwrap();
        assert_eq!(result, "Processed: hello");
    }

    #[tokio::test]
    async fn test_arc_tool_wrapper() {
        let tool = Arc::new(TestTool);
        let wrapper = ArcToolWrapper::new(tool);

        assert_eq!(wrapper.name(), "test_tool");

        let args_str = serde_json::to_string(&json!({"input": "arc_test"})).unwrap();
        let result = wrapper.call(args_str).await.unwrap();
        assert_eq!(result, "Processed: arc_test");
    }

    #[tokio::test]
    async fn test_tool_wrapper() {
        let wrapper = ToolWrapper::from_gasket(Box::new(TestTool));

        assert_eq!(wrapper.name(), "test_tool");

        let def = wrapper.definition().await;
        assert_eq!(def.name, "test_tool");

        let result = wrapper.call(json!({"input": "test"})).await.unwrap();
        assert_eq!(result, "Processed: test");
    }
}