//! DeepSeek LLM provider (OpenAI-compatible)

use crate::providers::{ChatRequest, ChatResponse, LlmProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use tracing::debug;

/// DeepSeek provider using OpenAI-compatible API
pub struct DeepSeekProvider {
    /// HTTP client
    client: Client,

    /// API key
    api_key: String,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,
}

impl DeepSeekProvider {
    /// Create a new DeepSeek provider
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_base: "https://api.deepseek.com/v1".to_string(),
            default_model: "deepseek-chat".to_string(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_base,
            default_model: "deepseek-chat".to_string(),
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Build chat completion request
    fn build_request(&self, request: ChatRequest) -> Value {
        let body = serde_json::to_value(&request).unwrap();

        // Add DeepSeek-specific parameters if needed
        // DeepSeek API is fully OpenAI-compatible

        debug!(
            "DeepSeek request: {}",
            serde_json::to_string_pretty(&body).unwrap()
        );
        body
    }

    /// Parse chat completion response
    fn parse_response(&self, response: Value) -> Result<ChatResponse> {
        debug!(
            "DeepSeek response: {}",
            serde_json::to_string_pretty(&response).unwrap()
        );

        // DeepSeek uses OpenAI-compatible response format
        let choices = response["choices"]
            .as_array()
            .ok_or_else(|| anyhow!("No choices in response"))?;

        if choices.is_empty() {
            return Err(anyhow!("Empty choices in response"));
        }

        let first_choice = &choices[0];
        let message = &first_choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());

        // Parse tool calls if present
        let tool_calls = if let Some(calls) = message["tool_calls"].as_array() {
            let parsed: Result<Vec<_>> = calls
                .iter()
                .map(|call| {
                    let id = call["id"].as_str().unwrap_or("").to_string();
                    let function = &call["function"];
                    let name = function["name"].as_str().unwrap_or("").to_string();
                    let arguments_str = function["arguments"].as_str().unwrap_or("{}");

                    // Parse arguments from JSON string
                    let arguments = serde_json::from_str(arguments_str)
                        .unwrap_or_else(|_| serde_json::json!({}));

                    Ok(crate::providers::ToolCall::new(id, name, arguments))
                })
                .collect();
            parsed?
        } else {
            vec![]
        };

        let has_tool_calls = !tool_calls.is_empty();

        // Check for reasoning content (DeepSeek R1 models)
        let reasoning_content = message["reasoning_content"].as_str().map(|s| s.to_string());

        Ok(ChatResponse {
            content,
            tool_calls,
            has_tool_calls,
            reasoning_content,
        })
    }
}

#[async_trait]
impl LlmProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        let body = self.build_request(request);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        self.parse_response(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "deepseek");
        assert_eq!(provider.default_model(), "deepseek-chat");
    }

    #[test]
    fn test_custom_api_base() {
        let provider = DeepSeekProvider::with_api_base(
            "test-key".to_string(),
            "https://custom.api.com/v1".to_string(),
        );
        assert_eq!(provider.api_base, "https://custom.api.com/v1");
    }

    #[test]
    fn test_custom_model() {
        let provider =
            DeepSeekProvider::new("test-key".to_string()).with_model("deepseek-coder".to_string());
        assert_eq!(provider.default_model(), "deepseek-coder");
    }

    #[test]
    fn test_build_request() {
        let provider = DeepSeekProvider::new("test-key".to_string());
        let request = ChatRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![crate::providers::ChatMessage::user("Hello")],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
        };

        let body = provider.build_request(request);

        assert_eq!(body["model"], "deepseek-chat");
        // Use approximate comparison for floating point
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.01);
        assert_eq!(body["max_tokens"], 100);
    }
}
