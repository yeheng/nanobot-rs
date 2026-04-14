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

    async fn handle(
        &self,
        params: Value,
        ctx: &DispatcherContext,
    ) -> Result<Value, RpcError> {
        // Get the LLM provider from context
        let provider = ctx
            .provider
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("No LLM provider available"))?;

        // Deserialize params into ChatRequest
        let request: gasket_providers::ChatRequest =
            serde_json::from_value(params).map_err(|e| {
                RpcError::invalid_params(format!("Failed to parse ChatRequest: {}", e))
            })?;

        // Call the provider's chat method
        let response = provider.chat(request).await.map_err(|e| {
            RpcError::internal_error(format!("LLM provider error: {}", e))
        })?;

        // Track token usage if tracker is available
        if let Some(tracker) = &ctx.token_tracker {
            if let Some(usage) = &response.usage {
                let token_usage = gasket_types::token_tracker::TokenUsage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens,
                };
                // Cost is calculated elsewhere (e.g., by pricing config)
                // Pass 0.0 here since we don't have pricing info in this handler
                tracker.accumulate(&token_usage, 0.0);
            }
        }

        // Manually construct JSON response since ChatResponse doesn't implement Serialize
        let mut response_obj = serde_json::Map::new();
        if let Some(content) = &response.content {
            response_obj.insert("content".to_string(), serde_json::json!(content));
        }
        if let Some(reasoning) = &response.reasoning_content {
            response_obj.insert("reasoning_content".to_string(), serde_json::json!(reasoning));
        }
        if !response.tool_calls.is_empty() {
            response_obj.insert("tool_calls".to_string(), serde_json::json!(response.tool_calls));
        }
        if let Some(usage) = &response.usage {
            response_obj.insert("usage".to_string(), serde_json::json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "total_tokens": usage.total_tokens,
            }));
        }

        Ok(Value::Object(response_obj))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_providers::{ChatMessage, ChatRequest, MessageRole};
    use std::sync::Arc;

    // Mock provider for testing
    struct MockProvider;

    #[async_trait::async_trait]
    impl gasket_providers::LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        async fn chat(
            &self,
            _request: ChatRequest,
        ) -> Result<gasket_providers::ChatResponse, gasket_providers::ProviderError> {
            Ok(gasket_providers::ChatResponse {
                content: Some("Test response".to_string()),
                tool_calls: vec![],
                reasoning_content: None,
                usage: Some(gasket_providers::Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                }),
            })
        }
    }

    #[tokio::test]
    async fn test_llm_chat_handler_success() {
        let handler = LlmChatHandler;

        // Create a valid ChatRequest
        let request = ChatRequest {
            model: "test-model".to_string(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some("Hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let params = serde_json::to_value(request).unwrap();

        // Create context with mock provider
        let mut ctx = DispatcherContext::default();
        ctx.provider = Some(Arc::new(MockProvider));

        // Execute handler
        let result = handler.handle(params, &ctx).await;

        assert!(result.is_ok());
        let response_value = result.unwrap();
        assert_eq!(response_value["content"], "Test response");
        assert_eq!(response_value["usage"]["input_tokens"], 10);
        assert_eq!(response_value["usage"]["output_tokens"], 5);
        assert_eq!(response_value["usage"]["total_tokens"], 15);
    }

    #[tokio::test]
    async fn test_llm_chat_handler_no_provider() {
        let handler = LlmChatHandler;

        // Empty context (no provider)
        let ctx = DispatcherContext::default();
        let params = serde_json::json!({"model": "test"});

        // Should fail with internal error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("No LLM provider available"));
    }

    #[tokio::test]
    async fn test_llm_chat_handler_invalid_params() {
        let handler = LlmChatHandler;

        // Invalid params (missing required fields)
        let params = serde_json::json!({"invalid": "data"});

        let mut ctx = DispatcherContext::default();
        ctx.provider = Some(Arc::new(MockProvider));

        // Should fail with invalid params error
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.code == -32602); // Invalid params
    }

    #[tokio::test]
    async fn test_llm_chat_handler_token_tracking() {
        let handler = LlmChatHandler;

        let request = ChatRequest {
            model: "test-model".to_string(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some("Track my tokens".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let params = serde_json::to_value(request).unwrap();

        // Create context with provider and tracker
        let mut ctx = DispatcherContext::default();
        ctx.provider = Some(Arc::new(MockProvider));
        ctx.token_tracker = Some(Arc::new(gasket_types::token_tracker::TokenTracker::unlimited(
            "USD",
        )));

        // Execute handler
        let result = handler.handle(params, &ctx).await;
        assert!(result.is_ok());

        // Verify token tracking
        let tracker = ctx.token_tracker.unwrap();
        assert_eq!(tracker.input_tokens(), 10);
        assert_eq!(tracker.output_tokens(), 5);
        assert_eq!(tracker.total_tokens(), 15);
        assert_eq!(tracker.request_count(), 1);
    }
}
