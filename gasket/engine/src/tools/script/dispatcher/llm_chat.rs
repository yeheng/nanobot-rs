//! LLM chat completion callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;

/// Handler for `llm/chat` RPC method calls.
///
/// This handler processes LLM chat completion requests from scripts by:
/// 1. Deserializing params into a ChatRequest
/// 2. Calling the provider's chat method
/// 3. Tracking token usage if a tracker is available
/// 4. Returning the ChatResponse as JSON
pub struct LlmChatHandler;

#[async_trait]
impl RpcHandler for LlmChatHandler {
    fn method(&self) -> &str {
        "llm/chat"
    }

    fn required_permission(&self) -> Permission {
        Permission::LlmChat
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        // Get the LLM provider from context
        let provider = &ctx.engine.provider;

        // Deserialize params into ChatRequest
        let request: gasket_providers::ChatRequest = serde_json::from_value(params)
            .map_err(|e| RpcError::invalid_params(format!("Failed to parse ChatRequest: {}", e)))?;

        // Call the provider's chat method
        let response = provider
            .chat(request)
            .await
            .map_err(|e| RpcError::internal_error(format!("LLM provider error: {}", e)))?;

        // Track token usage
        let tracker = &ctx.engine.token_tracker;
        if let Some(usage) = &response.usage {
            let token_usage = gasket_types::token_tracker::TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
            };
            tracker.accumulate(&token_usage, 0.0);
        }

        // Manually construct JSON response since ChatResponse doesn't implement Serialize
        let mut response_obj = serde_json::Map::new();
        if let Some(content) = &response.content {
            response_obj.insert("content".to_string(), serde_json::json!(content));
        }
        if let Some(reasoning) = &response.reasoning_content {
            response_obj.insert(
                "reasoning_content".to_string(),
                serde_json::json!(reasoning),
            );
        }
        if !response.tool_calls.is_empty() {
            response_obj.insert(
                "tool_calls".to_string(),
                serde_json::json!(response.tool_calls),
            );
        }
        if let Some(usage) = &response.usage {
            response_obj.insert(
                "usage".to_string(),
                serde_json::json!({
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "total_tokens": usage.total_tokens,
                }),
            );
        }

        Ok(Value::Object(response_obj))
    }
}

