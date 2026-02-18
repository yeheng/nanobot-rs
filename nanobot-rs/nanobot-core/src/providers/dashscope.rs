//! DashScope (阿里云通义千问) provider
//!
//! Supports Qwen models via DashScope API in OpenAI-compatible mode

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall};

/// DashScope (阿里云通义千问) provider
pub struct DashScopeProvider {
    client: Client,
    api_key: String,
    default_model: String,
}

impl DashScopeProvider {
    /// Create a new DashScope provider
    ///
    /// # Arguments
    /// * `api_key` - DashScope API key
    /// * `default_model` - Optional default model (defaults to qwen-plus)
    pub fn new(api_key: impl Into<String>, default_model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            default_model: default_model.unwrap_or_else(|| "qwen-plus".to_string()),
        }
    }

    /// Create provider with Qwen-Turbo model (fast, cost-effective)
    pub fn qwen_turbo(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-turbo".to_string()))
    }

    /// Create provider with Qwen-Plus model (balanced)
    pub fn qwen_plus(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-plus".to_string()))
    }

    /// Create provider with Qwen-Max model (most capable)
    pub fn qwen_max(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-max".to_string()))
    }

    /// Create provider with Qwen-Max-LongContext model (supports long context)
    pub fn qwen_max_longcontext(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-max-longcontext".to_string()))
    }

    /// Create provider with Qwen-VL-Plus model (vision-language)
    pub fn qwen_vl_plus(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-vl-plus".to_string()))
    }

    /// Create provider with Qwen-VL-Max model (vision-language, most capable)
    pub fn qwen_vl_max(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Some("qwen-vl-max".to_string()))
    }

    const API_BASE: &'static str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
}

#[async_trait]
impl LlmProvider for DashScopeProvider {
    fn name(&self) -> &str {
        "dashscope"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", Self::API_BASE);

        // DashScope uses OpenAI-compatible format in compatible mode
        let dashscope_request = DashScopeRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!("Sending request to DashScope: {}", url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&dashscope_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("DashScope API error: {} - {}", status, body);
        }

        let dashscope_response: DashScopeResponse = response.json().await?;
        debug!(
            "Received response from DashScope with {} choices",
            dashscope_response.choices.len()
        );

        let choice = dashscope_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in DashScope response"))?;

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

// DashScope API types (OpenAI-compatible)

#[derive(Debug, Serialize)]
struct DashScopeRequest {
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
struct DashScopeResponse {
    choices: Vec<DashScopeChoice>,
}

#[derive(Debug, Deserialize)]
struct DashScopeChoice {
    message: DashScopeMessage,
}

#[derive(Debug, Deserialize)]
struct DashScopeMessage {
    content: Option<String>,
    tool_calls: Option<Vec<DashScopeToolCall>>,
}

#[derive(Debug, Deserialize)]
struct DashScopeToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: DashScopeFunctionCall,
}

#[derive(Debug, Deserialize)]
struct DashScopeFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashscope_provider_creation() {
        let provider = DashScopeProvider::new("test-api-key", None);
        assert_eq!(provider.name(), "dashscope");
        assert_eq!(provider.default_model(), "qwen-plus");
    }

    #[test]
    fn test_dashscope_provider_custom_model() {
        let provider = DashScopeProvider::new("test-key", Some("qwen-max".to_string()));
        assert_eq!(provider.default_model(), "qwen-max");
    }

    #[test]
    fn test_dashscope_qwen_turbo() {
        let provider = DashScopeProvider::qwen_turbo("test-key");
        assert_eq!(provider.default_model(), "qwen-turbo");
    }

    #[test]
    fn test_dashscope_qwen_plus() {
        let provider = DashScopeProvider::qwen_plus("test-key");
        assert_eq!(provider.default_model(), "qwen-plus");
    }

    #[test]
    fn test_dashscope_qwen_max() {
        let provider = DashScopeProvider::qwen_max("test-key");
        assert_eq!(provider.default_model(), "qwen-max");
    }

    #[test]
    fn test_dashscope_qwen_max_longcontext() {
        let provider = DashScopeProvider::qwen_max_longcontext("test-key");
        assert_eq!(provider.default_model(), "qwen-max-longcontext");
    }

    #[test]
    fn test_dashscope_qwen_vl_plus() {
        let provider = DashScopeProvider::qwen_vl_plus("test-key");
        assert_eq!(provider.default_model(), "qwen-vl-plus");
    }

    #[test]
    fn test_dashscope_qwen_vl_max() {
        let provider = DashScopeProvider::qwen_vl_max("test-key");
        assert_eq!(provider.default_model(), "qwen-vl-max");
    }

    #[test]
    fn test_dashscope_api_base() {
        assert_eq!(
            DashScopeProvider::API_BASE,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"prompt": "写一首诗", "length": 100}"#;
        let result = parse_json_args(args);
        assert_eq!(result["prompt"], "写一首诗");
        assert_eq!(result["length"], 100);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "not valid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }
}
