//! Memory write callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;
use crate::tools::ToolContext;

/// Handler for `memory/write` RPC method calls.
///
/// This handler processes memory write requests from scripts by delegating
/// to the engine's memorize tool. Note: the tool name is "memorize" not "memory_write".
pub struct MemoryWriteHandler;

#[async_trait]
impl RpcHandler for MemoryWriteHandler {
    fn method(&self) -> &str {
        "memory/write"
    }

    fn required_permission(&self) -> Permission {
        Permission::MemoryWrite
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        // Get the tool registry from context
        let registry = ctx
            .tool_registry
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("No tool registry available"))?;

        // Build ToolContext with session key if available
        let mut tool_ctx = ToolContext::default();
        if let Some(ref key) = ctx.session_key {
            tool_ctx = tool_ctx.session_key(key.clone());
        }
        if let Some(ref tx) = ctx.outbound_tx {
            tool_ctx = tool_ctx.outbound_tx(tx.clone());
        }
        if let Some(ref spawner) = ctx.spawner {
            tool_ctx = tool_ctx.spawner(spawner.clone());
        }
        if let Some(ref tracker) = ctx.token_tracker {
            tool_ctx = tool_ctx.token_tracker(tracker.clone());
        }

        // Execute the memorize tool (note: tool name is "memorize" not "memory_write")
        let output = registry
            .execute("memorize", params, &tool_ctx)
            .await
            .map_err(|e| RpcError::internal_error(format!("Memory write failed: {}", e)))?;

        // Wrap result in JSON object
        Ok(serde_json::json!({"output": output}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{MemorizeTool, ToolRegistry};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_memory_write_handler_success() {
        let handler = MemoryWriteHandler;

        // Create a temporary memory directory
        let tmp = TempDir::new().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        // Create tool registry and register memorize
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MemorizeTool::with_dir(tmp.path().to_path_buf())));

        // Create context with tool registry
        let mut ctx = DispatcherContext::default();
        ctx.tool_registry = Some(Arc::new(registry));

        // Execute handler with memory write
        let params = serde_json::json!({
            "title": "Test Memory",
            "content": "This is a test memory",
            "tags": ["test", "example"]
        });
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response["output"].is_string());
        let output = response["output"].as_str().unwrap();
        assert!(output.contains("saved") || output.contains("Success"));
    }

    #[tokio::test]
    async fn test_memory_write_handler_no_registry() {
        let handler = MemoryWriteHandler;

        // Empty context (no tool registry)
        let ctx = DispatcherContext::default();
        let params = serde_json::json!({
            "title": "Test",
            "content": "Test content"
        });

        // Should fail with internal error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("No tool registry available"));
    }

    #[tokio::test]
    async fn test_memory_write_handler_with_tags() {
        let handler = MemoryWriteHandler;

        // Create tool registry
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MemorizeTool::with_dir(
            tempfile::TempDir::new().unwrap().path().to_path_buf(),
        )));

        // Create context with tool registry
        let mut ctx = DispatcherContext::default();
        ctx.tool_registry = Some(Arc::new(registry));

        // Execute handler with tags
        let params = serde_json::json!({
            "title": "Tagged Memory",
            "content": "Memory with tags",
            "tags": ["rust", "programming"]
        });
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response["output"].is_string());
    }
}
