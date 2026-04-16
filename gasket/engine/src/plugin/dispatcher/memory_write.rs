//! Memory write callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::plugin::manifest::Permission;
use crate::plugin::rpc::RpcError;
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
        let registry = &ctx.engine.tool_registry;

        let tool_ctx = ToolContext::default()
            .session_key(ctx.engine.session_key.clone())
            .outbound_tx(ctx.engine.outbound_tx.clone())
            .spawner(ctx.engine.spawner.clone())
            .token_tracker(ctx.engine.token_tracker.clone());

        let output = registry
            .execute("memorize", params, &tool_ctx)
            .await
            .map_err(|e| RpcError::internal_error(format!("Memory write failed: {}", e)))?;

        Ok(serde_json::json!({"output": output}))
    }
}
