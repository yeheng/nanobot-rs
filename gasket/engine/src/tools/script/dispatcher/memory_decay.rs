//! Memory decay callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;
use crate::tools::ToolContext;

/// Handler for `memory/decay` RPC method calls.
///
/// This handler processes memory decay requests from scripts by delegating
/// to the engine's memory_decay tool.
pub struct MemoryDecayHandler;

#[async_trait]
impl RpcHandler for MemoryDecayHandler {
    fn method(&self) -> &str {
        "memory/decay"
    }

    fn required_permission(&self) -> Permission {
        Permission::MemoryDecay
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let registry = &ctx.engine.tool_registry;

        let tool_ctx = ToolContext::default()
            .session_key(ctx.engine.session_key.clone())
            .outbound_tx(ctx.engine.outbound_tx.clone())
            .spawner(ctx.engine.spawner.clone())
            .token_tracker(ctx.engine.token_tracker.clone());

        let output = registry
            .execute("memory_decay", params, &tool_ctx)
            .await
            .map_err(|e| RpcError::internal_error(format!("Memory decay failed: {}", e)))?;

        Ok(serde_json::json!({"output": output}))
    }
}
