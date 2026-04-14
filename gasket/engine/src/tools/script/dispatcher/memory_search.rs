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
        let registry = &ctx.engine.tool_registry;

        // Build ToolContext from engine handle
        let tool_ctx = ToolContext::default()
            .session_key(ctx.engine.session_key.clone())
            .outbound_tx(ctx.engine.outbound_tx.clone())
            .spawner(ctx.engine.spawner.clone())
            .token_tracker(ctx.engine.token_tracker.clone());

        // Execute the memory_search tool
        let output = registry
            .execute("memory_search", params, &tool_ctx)
            .await
            .map_err(|e| RpcError::internal_error(format!("Memory search failed: {}", e)))?;

        Ok(serde_json::json!({"output": output}))
    }
}

