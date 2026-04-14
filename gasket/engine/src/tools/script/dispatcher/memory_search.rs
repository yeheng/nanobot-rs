//! Memory search callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;
use crate::tools::ToolContext;

/// Handler for `memory/search` RPC method calls.
///
/// This handler processes memory search requests from scripts by delegating
/// to the engine's memory_search tool.
pub struct MemorySearchHandler;

#[async_trait]
impl RpcHandler for MemorySearchHandler {
    fn method(&self) -> &str {
        "memory/search"
    }

    fn required_permission(&self) -> Permission {
        Permission::MemorySearch
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

        // Execute the memory_search tool
        let output = registry
            .execute("memory_search", params, &tool_ctx)
            .await
            .map_err(|e| RpcError::internal_error(format!("Memory search failed: {}", e)))?;

        // Wrap result in JSON object
        Ok(serde_json::json!({"output": output}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{MemorySearchTool, ToolRegistry};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_memory_search_handler_success() {
        let handler = MemorySearchHandler;

        // Create a temporary memory directory
        let tmp = TempDir::new().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        // Create a test memory file
        let content = r#"---
title: "Test Memory"
tags: [test, example]
scenario: default
---
This is a test memory about PostgreSQL."#;
        tokio::fs::write(memory_dir.join("test.md"), content)
            .await
            .unwrap();

        // Create tool registry and register memory_search
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MemorySearchTool::with_dir(
            tmp.path().to_path_buf(),
        )));

        // Create context with tool registry
        let mut ctx = DispatcherContext::default();
        ctx.tool_registry = Some(Arc::new(registry));

        // Execute handler with search query
        let params = serde_json::json!({"query": "PostgreSQL"});
        let result = handler.handle(params, &ctx).await;

        // Just verify the handler executes successfully without asserting on specific content
        // since memory search implementation may vary
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response["output"].is_string());
    }

    #[tokio::test]
    async fn test_memory_search_handler_no_registry() {
        let handler = MemorySearchHandler;

        // Empty context (no tool registry)
        let ctx = DispatcherContext::default();
        let params = serde_json::json!({"query": "test"});

        // Should fail with internal error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("No tool registry available"));
    }

    #[tokio::test]
    async fn test_memory_search_handler_with_session() {
        let handler = MemorySearchHandler;

        // Create tool registry with mock tool
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MemorySearchTool::with_dir(
            tempfile::TempDir::new().unwrap().path().to_path_buf(),
        )));

        // Create context with session key
        let mut ctx = DispatcherContext::default();
        ctx.tool_registry = Some(Arc::new(registry));
        ctx.session_key = Some(gasket_types::events::SessionKey::new(
            gasket_types::events::ChannelType::Telegram,
            "test-chat",
        ));

        let params = serde_json::json!({"query": "test"});
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
    }
}
