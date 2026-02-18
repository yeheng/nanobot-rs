//! Zhipu AI (智谱) GLM provider
//!
//! Supports GLM-5 and GLM-4.7-flash models via Zhipu AI API

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall};

/// Zhipu AI (智谱) provider
pub struct ZhipuProvider {
    api_base: String,
    client: Client,
    api_key: String,
    default_model: String,
}

impl ZhipuProvider {
    /// Create a new Zhipu provider
    ///
    /// # Arguments
    /// * `api_key` - Zhipu AI JWT token
    /// * `default_model` - Optional default model (defaults to GLM-5)
    pub fn new(
        api_key: impl Into<String>,
        default_api_base: Option<String>,
        default_model: Option<String>,
    ) -> Self {
        Self {
            api_base: default_api_base.unwrap_or_else(|| Self::API_BASE.to_string()),
            client: Client::new(),
            api_key: api_key.into(),
            default_model: default_model.unwrap_or_else(|| "GLM-5".to_string()),
        }
    }

    /// Create provider with GLM-5-Plus model
    pub fn glm_5(api_key: impl Into<String>) -> Self {
        Self::new(
            api_key,
            Some(Self::API_BASE.to_string()),
            Some("GLM-5".to_string()),
        )
    }

    /// Create provider with GLM-5-Plus model
    pub fn glm_4_7(api_key: impl Into<String>) -> Self {
        Self::new(
            api_key,
            Some(Self::API_BASE.to_string()),
            Some("GLM-4.7".to_string()),
        )
    }

    const API_BASE: &'static str = "https://open.bigmodel.cn/api/paas/v4";
}

#[async_trait]
impl LlmProvider for ZhipuProvider {
    fn name(&self) -> &str {
        "zhipu"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        // Zhipu uses OpenAI-compatible format
        let zhipu_request = ZhipuRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!("Sending request to Zhipu AI: {}", url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&zhipu_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Zhipu API error: {} - {}", status, body);
        }

        let zhipu_response: ZhipuResponse = response.json().await?;
        debug!(
            "Received response from Zhipu with {} choices",
            zhipu_response.choices.len()
        );

        let choice = zhipu_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in Zhipu response"))?;

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

// Zhipu API types (OpenAI-compatible)

#[derive(Debug, Serialize)]
struct ZhipuRequest {
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
struct ZhipuResponse {
    choices: Vec<ZhipuChoice>,
}

#[derive(Debug, Deserialize)]
struct ZhipuChoice {
    message: ZhipuMessage,
}

#[derive(Debug, Deserialize)]
struct ZhipuMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ZhipuToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ZhipuToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: ZhipuFunctionCall,
}

#[derive(Debug, Deserialize)]
struct ZhipuFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zhipu_provider_creation() {
        let provider = ZhipuProvider::new("test-jwt-token", None, None);
        assert_eq!(provider.name(), "zhipu");
        assert_eq!(provider.default_model(), "GLM-5");
    }

    #[test]
    fn test_zhipu_provider_custom_model() {
        let provider = ZhipuProvider::new("test-token", None, Some("GLM-5-plus".to_string()));
        assert_eq!(provider.default_model(), "GLM-5-plus");
    }

    #[test]
    fn test_zhipu_api_base() {
        assert_eq!(
            ZhipuProvider::API_BASE,
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }

    #[test]
    fn test_parse_json_args() {
        let args = r#"{"query": "你好", "top_k": 5}"#;
        let result = parse_json_args(args);
        assert_eq!(result["query"], "你好");
        assert_eq!(result["top_k"], 5);
    }

    #[test]
    fn test_parse_json_args_invalid() {
        let args = "invalid json";
        let result = parse_json_args(args);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }
}
