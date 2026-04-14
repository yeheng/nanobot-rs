//! Subagent spawn callback handler.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;

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
        // Get the spawner from context
        let spawner = ctx
            .spawner
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("No subagent spawner available"))?;

        // Deserialize params into SpawnRequest
        let request: SpawnRequest = serde_json::from_value(params).map_err(|e| {
            RpcError::invalid_params(format!("Failed to parse SpawnRequest: {}", e))
        })?;

        // Call the spawner
        let result = spawner
            .spawn(request.task, request.model_id)
            .await
            .map_err(|e| RpcError::internal_error(format!("Subagent spawn failed: {}", e)))?;

        // Build response
        let response = SpawnResponse {
            id: result.id,
            task: result.task,
            content: result.response.content,
            model: result.model,
        };

        // Serialize to JSON
        serde_json::to_value(response)
            .map_err(|e| RpcError::internal_error(format!("Failed to serialize response: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::{SubagentResponse, SubagentResult, SubagentSpawner};
    use std::sync::Arc;

    // Mock spawner for testing
    struct MockSpawner;

    #[async_trait::async_trait]
    impl SubagentSpawner for MockSpawner {
        async fn spawn(
            &self,
            task: String,
            model_id: Option<String>,
        ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
            Ok(SubagentResult {
                id: "test-subagent-123".to_string(),
                task,
                response: SubagentResponse {
                    content: "Mock subagent response".to_string(),
                    reasoning_content: None,
                    tools_used: vec![],
                    model: model_id.clone(),
                    token_usage: None,
                    cost: 0.0,
                },
                model: model_id,
            })
        }
    }

    #[tokio::test]
    async fn test_subagent_spawn_handler_success() {
        let handler = SubagentSpawnHandler;

        // Create valid spawn request
        let request = SpawnRequest {
            task: "Test task".to_string(),
            model_id: Some("test-model".to_string()),
        };
        let params = serde_json::to_value(request).unwrap();

        // Create context with mock spawner
        let mut ctx = DispatcherContext::default();
        ctx.spawner = Some(Arc::new(MockSpawner));

        // Execute handler
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
        let response_value = result.unwrap();
        assert_eq!(response_value["id"], "test-subagent-123");
        assert_eq!(response_value["task"], "Test task");
        assert_eq!(response_value["content"], "Mock subagent response");
        assert_eq!(response_value["model"], "test-model");
    }

    #[tokio::test]
    async fn test_subagent_spawn_handler_no_spawner() {
        let handler = SubagentSpawnHandler;

        // Empty context (no spawner)
        let ctx = DispatcherContext::default();
        let params = serde_json::json!({"task": "test"});

        // Should fail with internal error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("No subagent spawner available"));
    }

    #[tokio::test]
    async fn test_subagent_spawn_handler_invalid_params() {
        let handler = SubagentSpawnHandler;

        // Invalid params (missing required task field)
        let params = serde_json::json!({"invalid": "data"});

        let mut ctx = DispatcherContext::default();
        ctx.spawner = Some(Arc::new(MockSpawner));

        // Should fail with invalid params error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.code == -32602); // Invalid params
    }

    #[tokio::test]
    async fn test_subagent_spawn_handler_no_model() {
        let handler = SubagentSpawnHandler;

        // Request without model_id
        let request = SpawnRequest {
            task: "Task without model".to_string(),
            model_id: None,
        };
        let params = serde_json::to_value(request).unwrap();

        let mut ctx = DispatcherContext::default();
        ctx.spawner = Some(Arc::new(MockSpawner));

        // Execute handler
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
        let response_value = result.unwrap();
        assert_eq!(response_value["id"], "test-subagent-123");
        assert_eq!(response_value["task"], "Task without model");
        assert_eq!(response_value["model"], Value::Null);
    }
}
