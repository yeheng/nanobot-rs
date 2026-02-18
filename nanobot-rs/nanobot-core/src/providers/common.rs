//! Common provider functionality for OpenAI-compatible APIs
//!
//! This module provides shared functionality for providers that implement
//! the OpenAI-compatible API format, reducing code duplication.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{ChatMessage, ChatRequest, ChatResponse, ToolCall, ToolDefinition};

/// Common provider configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// API base URL
    pub api_base: String,
    /// API key
    pub api_key: String,
    /// Default model
    pub default_model: String,
}

/// OpenAI-compatible provider base implementation
///
/// This struct provides the common HTTP client and request/response handling
/// for providers that use the OpenAI-compatible API format.
pub struct OpenAICompatibleProvider {
    client: Client,
    config: ProviderConfig,
    name: &'static str,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    pub fn new(name: &'static str, config: ProviderConfig) -> Self {
        Self {
            client: Client::new(),
            config,
            name,
        }
    }

    /// Create with custom client
    pub fn with_client(name: &'static str, config: ProviderConfig, client: Client) -> Self {
        Self {
            client,
            config,
            name,
        }
    }

    /// Get the provider name
    pub fn name(&self) -> &str {
        self.name
    }

    /// Get the default model
    pub fn default_model(&self) -> &str {
        &self.config.default_model
    }

    /// Get the API base URL
    pub fn api_base(&self) -> &str {
        &self.config.api_base
    }

    /// Get the API key
    pub fn api_key(&self) -> &str {
        &self.config.api_key
    }

    /// Send a chat completion request
    pub async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.config.api_base);

        let openai_request = OpenAICompatibleRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!("Sending request to {} API: {}", self.name, url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&openai_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("{} API error: {} - {}", self.name, status, body);
        }

        let api_response: OpenAICompatibleResponse = response.json().await?;
        debug!(
            "Received response from {} with {} choices",
            self.name,
            api_response.choices.len()
        );

        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in {} response", self.name))?;

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                ToolCall::new(
                    tc.id,
                    tc.function.name,
                    parse_json_args(&tc.function.arguments),
                )
            })
            .collect();

        let has_tool_calls = !tool_calls.is_empty();

        Ok(ChatResponse {
            content: choice.message.content,
            tool_calls,
            has_tool_calls,
            reasoning_content: None,
        })
    }
}

/// Parse JSON arguments from string
pub fn parse_json_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}))
}

// OpenAI-compatible API types

#[derive(Debug, Serialize)]
struct OpenAICompatibleRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleResponse {
    choices: Vec<OpenAICompatibleChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleChoice {
    message: OpenAICompatibleMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAICompatibleToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAICompatibleFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_creation() {
        let config = ProviderConfig {
            api_base: "https://api.example.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "test-model".to_string(),
        };

        assert_eq!(config.api_base, "https://api.example.com/v1");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.default_model, "test-model");
    }

    #[test]
    fn test_provider_creation() {
        let config = ProviderConfig {
            api_base: "https://api.example.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "test-model".to_string(),
        };

        let provider = OpenAICompatibleProvider::new("test", config);

        assert_eq!(provider.name(), "test");
        assert_eq!(provider.default_model(), "test-model");
        assert_eq!(provider.api_base(), "https://api.example.com/v1");
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"key": "value", "number": 42}"#;
        let result = parse_json_args(args);
        assert_eq!(result["key"], "value");
        assert_eq!(result["number"], 42);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "not valid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_parse_json_args_empty() {
        let args = "";
        let result = parse_json_args(args);
        assert!(result.is_object());
    }
}
