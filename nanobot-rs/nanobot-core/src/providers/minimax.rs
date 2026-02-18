//! MiniMax AI provider
//!
//! Supports MiniMax's large language models via OpenAI-compatible API

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall};

/// MiniMax AI provider
///
/// MiniMax offers various models including:
/// - abab6.5-chat: Latest flagship model
/// - abab6.5s-chat: Faster, more cost-effective
/// - abab5.5-chat: Previous generation model
/// - abab5.5s-chat: Lightweight version
pub struct MiniMaxProvider {
    api_base: String,
    client: Client,
    api_key: String,
    group_id: Option<String>,
    default_model: String,
}

impl MiniMaxProvider {
    /// MiniMax API base URL
    const API_BASE: &'static str = "https://api.minimax.chat/v1";

    /// Create a new MiniMax provider
    ///
    /// # Arguments
    /// * `api_key` - MiniMax API key
    /// * `default_model` - Optional default model (defaults to abab6.5-chat)
    pub fn new(api_key: impl Into<String>, default_model: Option<String>) -> Self {
        Self {
            api_base: Self::API_BASE.to_string(),
            client: Client::new(),
            api_key: api_key.into(),
            group_id: None,
            default_model: default_model.unwrap_or_else(|| "abab6.5-chat".to_string()),
        }
    }

    /// Create provider with group ID (required for some API operations)
    pub fn with_group_id(mut self, group_id: impl Into<String>) -> Self {
        self.group_id = Some(group_id.into());
        self
    }

    /// Create provider with abab6.5-chat model (flagship)
    pub fn abab6_5(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("abab6.5-chat".to_string()))
    }

    /// Create provider with abab6.5s-chat model (fast)
    pub fn abab6_5s(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("abab6.5s-chat".to_string()))
    }

    /// Create provider with abab5.5-chat model
    pub fn abab5_5(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("abab5.5-chat".to_string()))
    }

    /// Create provider with abab5.5s-chat model (lightweight)
    pub fn abab5_5s(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("abab5.5s-chat".to_string()))
    }

    /// Get the group ID if set
    pub fn group_id(&self) -> Option<&str> {
        self.group_id.as_deref()
    }
}

#[async_trait]
impl LlmProvider for MiniMaxProvider {
    fn name(&self) -> &str {
        "minimax"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        // MiniMax uses OpenAI-compatible format
        let minimax_request = MiniMaxRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            // MiniMax-specific: can add tokens_to_generate for precise control
        };

        debug!("Sending request to MiniMax: {}", url);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&minimax_request);

        // Add group ID header if available
        if let Some(ref group_id) = self.group_id {
            req = req.header("X-Group-Id", group_id);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("MiniMax API error: {} - {}", status, body);
        }

        let minimax_response: MiniMaxResponse = response.json().await?;
        debug!(
            "Received response from MiniMax with {} choices",
            minimax_response.choices.len()
        );

        let choice = minimax_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in MiniMax response"))?;

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

// MiniMax API types (OpenAI-compatible)

#[derive(Debug, Serialize)]
struct MiniMaxRequest {
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
struct MiniMaxResponse {
    choices: Vec<MiniMaxChoice>,
    #[allow(dead_code)]
    usage: Option<MiniMaxUsage>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxChoice {
    message: MiniMaxMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxMessage {
    content: Option<String>,
    tool_calls: Option<Vec<MiniMaxToolCall>>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: MiniMaxFunctionCall,
}

#[derive(Debug, Deserialize)]
struct MiniMaxFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct MiniMaxUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimax_provider_creation() {
        let provider = MiniMaxProvider::new("test-api-key", None);
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), "abab6.5-chat");
    }

    #[test]
    fn test_minimax_provider_custom_model() {
        let provider = MiniMaxProvider::new("test-key", Some("abab5.5s-chat".to_string()));
        assert_eq!(provider.default_model(), "abab5.5s-chat");
    }

    #[test]
    fn test_minimax_abab6_5() {
        let provider = MiniMaxProvider::abab6_5("test-key");
        assert_eq!(provider.default_model(), "abab6.5-chat");
    }

    #[test]
    fn test_minimax_abab6_5s() {
        let provider = MiniMaxProvider::abab6_5s("test-key");
        assert_eq!(provider.default_model(), "abab6.5s-chat");
    }

    #[test]
    fn test_minimax_abab5_5() {
        let provider = MiniMaxProvider::abab5_5("test-key");
        assert_eq!(provider.default_model(), "abab5.5-chat");
    }

    #[test]
    fn test_minimax_with_group_id() {
        let provider = MiniMaxProvider::new("test-key", None).with_group_id("group123");
        assert_eq!(provider.group_id(), Some("group123"));
    }

    #[test]
    fn test_minimax_api_base() {
        assert_eq!(MiniMaxProvider::API_BASE, "https://api.minimax.chat/v1");
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"prompt": "Hello", "max_tokens": 100}"#;
        let result = parse_json_args(args);
        assert_eq!(result["prompt"], "Hello");
        assert_eq!(result["max_tokens"], 100);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "not valid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }
}
