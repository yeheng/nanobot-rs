//! Moonshot AI (Kimi) provider
//!
//! Supports Moonshot models with long context windows

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall};

/// Moonshot AI (Kimi) provider
pub struct MoonshotProvider {
    client: Client,
    api_key: String,
    default_model: String,
}

impl MoonshotProvider {
    /// Create a new Moonshot provider
    ///
    /// # Arguments
    /// * `api_key` - Moonshot AI API key
    /// * `default_model` - Optional default model (defaults to moonshot-v1-8k)
    pub fn new(api_key: impl Into<String>, default_model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            default_model: default_model.unwrap_or_else(|| "moonshot-v1-8k".to_string()),
        }
    }

    /// Create provider with Moonshot V1 8K context model
    pub fn moonshot_v1_8k(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("moonshot-v1-8k".to_string()))
    }

    /// Create provider with Moonshot V1 32K context model
    pub fn moonshot_v1_32k(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("moonshot-v1-32k".to_string()))
    }

    /// Create provider with Moonshot V1 128K context model (longest context)
    pub fn moonshot_v1_128k(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("moonshot-v1-128k".to_string()))
    }

    const API_BASE: &'static str = "https://api.moonshot.cn/v1";
}

#[async_trait]
impl LlmProvider for MoonshotProvider {
    fn name(&self) -> &str {
        "moonshot"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", Self::API_BASE);

        // Moonshot uses OpenAI-compatible format
        let moonshot_request = MoonshotRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!("Sending request to Moonshot AI: {}", url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&moonshot_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Moonshot API error: {} - {}", status, body);
        }

        let moonshot_response: MoonshotResponse = response.json().await?;
        debug!(
            "Received response from Moonshot with {} choices",
            moonshot_response.choices.len()
        );

        let choice = moonshot_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in Moonshot response"))?;

        // Parse tool calls
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
fn parse_json_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}))
}

// Moonshot API types (OpenAI-compatible)

#[derive(Debug, Serialize)]
struct MoonshotRequest {
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
struct MoonshotResponse {
    choices: Vec<MoonshotChoice>,
}

#[derive(Debug, Deserialize)]
struct MoonshotChoice {
    message: MoonshotMessage,
}

#[derive(Debug, Deserialize)]
struct MoonshotMessage {
    content: Option<String>,
    tool_calls: Option<Vec<MoonshotToolCall>>,
}

#[derive(Debug, Deserialize)]
struct MoonshotToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: MoonshotFunctionCall,
}

#[derive(Debug, Deserialize)]
struct MoonshotFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moonshot_provider_creation() {
        let provider = MoonshotProvider::new("test-api-key", None);
        assert_eq!(provider.name(), "moonshot");
        assert_eq!(provider.default_model(), "moonshot-v1-8k");
    }

    #[test]
    fn test_moonshot_provider_custom_model() {
        let provider = MoonshotProvider::new("test-key", Some("moonshot-v1-128k".to_string()));
        assert_eq!(provider.default_model(), "moonshot-v1-128k");
    }

    #[test]
    fn test_moonshot_v1_8k() {
        let provider = MoonshotProvider::moonshot_v1_8k("test-key");
        assert_eq!(provider.default_model(), "moonshot-v1-8k");
    }

    #[test]
    fn test_moonshot_v1_32k() {
        let provider = MoonshotProvider::moonshot_v1_32k("test-key");
        assert_eq!(provider.default_model(), "moonshot-v1-32k");
    }

    #[test]
    fn test_moonshot_v1_128k() {
        let provider = MoonshotProvider::moonshot_v1_128k("test-key");
        assert_eq!(provider.default_model(), "moonshot-v1-128k");
    }

    #[test]
    fn test_moonshot_api_base() {
        assert_eq!(
            MoonshotProvider::API_BASE,
            "https://api.moonshot.cn/v1"
        );
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"context": "长文本", "summary": true}"#;
        let result = parse_json_args(args);
        assert_eq!(result["context"], "长文本");
        assert_eq!(result["summary"], true);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "not valid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }
}
