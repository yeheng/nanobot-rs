//! Subagent spawn callback handler.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::plugin::manifest::Permission;
use crate::plugin::rpc::RpcError;

/// Handler for `subagent/spawn` RPC method calls.
///
/// This handler processes subagent spawn requests from scripts by delegating
/// to the SubagentSpawner in the context.
pub struct SubagentSpawnHandler;

/// Request parameters for spawning a subagent.
#[derive(Debug, Deserialize, Serialize)]
struct SpawnRequest {
    /// Task description for the subagent
    task: String,
    /// Optional model profile ID to use
    model_id: Option<String>,
}

/// Response from spawning a subagent.
#[derive(Debug, Serialize)]
struct SpawnResponse {
    /// Subagent session ID
    id: String,
    /// Task that was executed
    task: String,
    /// Response content from the subagent
    content: String,
    /// Model used for execution
    model: Option<String>,
}

#[async_trait]
impl RpcHandler for SubagentSpawnHandler {
    fn method(&self) -> &str {
        "subagent/spawn"
    }

    fn required_permission(&self) -> Permission {
        Permission::SubagentSpawn
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let spawner = &ctx.engine.spawner;

        let request: SpawnRequest = serde_json::from_value(params).map_err(|e| {
            RpcError::invalid_params(format!("Failed to parse SpawnRequest: {}", e))
        })?;

        let result = spawner
            .spawn(request.task, request.model_id)
            .await
            .map_err(|e| RpcError::internal_error(format!("Subagent spawn failed: {}", e)))?;

        let response = SpawnResponse {
            id: result.id,
            task: result.task,
            content: result.response.content,
            model: result.model,
        };

        serde_json::to_value(response)
            .map_err(|e| RpcError::internal_error(format!("Failed to serialize response: {}", e)))
    }
}
