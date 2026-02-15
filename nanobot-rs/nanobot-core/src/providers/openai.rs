//! OpenAI-compatible API provider

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall};

/// OpenAI-compatible provider
pub struct OpenAIProvider {
    client: Client,
    api_base: String,
    api_key: String,
    default_model: String,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider
    pub fn new(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            api_base: api_base.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            default_model: default_model.unwrap_or_else(|| "gpt-4o".to_string()),
        }
    }

    /// Create an OpenRouter provider
    pub fn openrouter(api_key: impl Into<String>) -> Self {
        Self::new(
            api_key,
            Some("https://openrouter.ai/api/v1".to_string()),
            Some("anthropic/claude-sonnet-4".to_string()),
        )
    }

    /// Create an Anthropic provider (via OpenAI-compatible endpoint)
    pub fn anthropic(api_key: impl Into<String>) -> Self {
        Self::new(
            api_key,
            Some("https://api.anthropic.com/v1".to_string()),
            Some("claude-sonnet-4-20250514".to_string()),
        )
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        // Convert our request to OpenAI format
        let openai_request = OpenAIRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!("Sending request to {}", url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&openai_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("API error: {} - {}", status, body);
        }

        let openai_response: OpenAIResponse = response.json().await?;
        debug!(
            "Received response with {} choices",
            openai_response.choices.len()
        );

        let choice = openai_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in response"))?;

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

/// Parse JSON arguments from string (OpenAI returns them as strings)
fn parse_json_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}))
}

// OpenAI API types

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<super::ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OpenAIProvider::new("test-key", None, None);
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model(), "gpt-4o");
    }

    #[test]
    fn test_openrouter_provider() {
        let provider = OpenAIProvider::openrouter("sk-or-test");
        assert_eq!(provider.api_base, "https://openrouter.ai/api/v1");
        assert_eq!(provider.default_model(), "anthropic/claude-sonnet-4");
    }

    #[test]
    fn test_anthropic_provider() {
        let provider = OpenAIProvider::anthropic("sk-ant-test");
        assert_eq!(provider.api_base, "https://api.anthropic.com/v1");
        assert_eq!(provider.default_model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_custom_provider() {
        let provider = OpenAIProvider::new(
            "custom-key",
            Some("https://custom.api.com/v1".to_string()),
            Some("custom-model".to_string()),
        );
        assert_eq!(provider.api_base, "https://custom.api.com/v1");
        assert_eq!(provider.default_model(), "custom-model");
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"path": "/tmp/test", "limit": 10}"#;
        let result = parse_json_args(args);
        assert_eq!(result["path"], "/tmp/test");
        assert_eq!(result["limit"], 10);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "not valid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
    }
}
