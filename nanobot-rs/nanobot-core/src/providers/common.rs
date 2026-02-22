//! Common provider functionality for OpenAI-compatible APIs
//!
//! This module provides a generic, reusable provider implementation for any
//! LLM service that speaks the OpenAI-compatible API format. Instead of
//! copy-pasting a new file per vendor, instantiate `OpenAICompatibleProvider`
//! with the right config.
//!
//! # Adding a new provider
//!
//! To add support for a new OpenAI-compatible provider, simply add an entry
//! to the `PROVIDER_DEFAULTS` map below. No code changes needed.

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, ToolCall, ToolDefinition};

/// Default configuration for known providers
///
/// This is data, not code. Adding a new provider just means adding a row here.
static PROVIDER_DEFAULTS: &[(&str, &str, Option<&str>)] = &[
    // (name, api_base, default_model)
    ("openai", "https://api.openai.com/v1", Some("gpt-4o")),
    ("openrouter", "https://openrouter.ai/api/v1", Some("anthropic/claude-sonnet-4")),
    ("anthropic", "https://api.anthropic.com/v1", Some("claude-sonnet-4-20250514")),
    ("dashscope", "https://dashscope.aliyuncs.com/compatible-mode/v1", Some("qwen-max")),
    ("moonshot", "https://api.moonshot.cn/v1", Some("kimi-k2.5")),
    ("zhipu", "https://open.bigmodel.cn/api/paas/v4", Some("glm-5")),
    ("zhipu_coding", "https://open.bigmodel.cn/api/coding/paas/v4", Some("glm-5")),
    ("minimax", "https://api.minimax.chat/v1", Some("M2.2")),
    ("deepseek", "https://api.deepseek.com/v1", Some("deepseek-chat")),
    ("ollama", "http://localhost:11434/v1", Some("llama3")),
];

/// Get default API base URL for a provider name
pub fn get_default_api_base(name: &str) -> Option<&'static str> {
    PROVIDER_DEFAULTS.iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, url, _)| *url)
}

/// Get default model for a provider name
pub fn get_default_model(name: &str) -> Option<&'static str> {
    PROVIDER_DEFAULTS.iter()
        .find(|(n, _, _)| *n == name)
        .and_then(|(_, _, model)| *model)
}

/// Common provider configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider display name (e.g. "openai", "dashscope")
    pub name: String,
    /// API base URL (e.g. "https://api.openai.com/v1")
    pub api_base: String,
    /// API key
    pub api_key: String,
    /// Default model
    pub default_model: String,
    /// Extra HTTP headers to send with every request
    pub extra_headers: HashMap<String, String>,
}

/// OpenAI-compatible provider that implements `LlmProvider`.
///
/// This single struct replaces per-vendor provider files (dashscope.rs,
/// moonshot.rs, zhipu.rs, minimax.rs, etc.) — all of which performed
/// identical HTTP POST + JSON parse logic.
pub struct OpenAICompatibleProvider {
    client: Client,
    config: ProviderConfig,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Create with custom HTTP client
    pub fn with_client(config: ProviderConfig, client: Client) -> Self {
        Self { client, config }
    }

    /// Create a provider by name, looking up defaults from PROVIDER_DEFAULTS.
    ///
    /// This is the recommended way to create providers. The URL and default model
    /// are looked up from the data table, so adding new providers requires no code changes.
    ///
    /// # Example
    /// ```ignore
    /// let provider = OpenAICompatibleProvider::from_name("dashscope", "your-api-key", None, None);
    /// ```
    pub fn from_name(
        name: &str,
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
    ) -> Self {
        let resolved_base = api_base
            .or_else(|| get_default_api_base(name).map(|s| s.to_string()))
            .unwrap_or_else(|| panic!("Unknown provider: {}. Add it to PROVIDER_DEFAULTS or provide api_base.", name));

        let resolved_model = default_model
            .or_else(|| get_default_model(name).map(|s| s.to_string()))
            .unwrap_or_else(|| "default".to_string());

        Self::new(ProviderConfig {
            name: name.to_string(),
            api_base: resolved_base,
            api_key: api_key.into(),
            default_model: resolved_model,
            extra_headers: HashMap::new(),
        })
    }

    /// Create a provider by name with extra headers (e.g., for MiniMax's X-Group-Id)
    pub fn from_name_with_headers(
        name: &str,
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        let mut provider = Self::from_name(name, api_key, api_base, default_model);
        provider.config.extra_headers = extra_headers;
        provider
    }

    // -- Convenience constructors for well-known providers (delegate to from_name) --

    /// Create an OpenAI provider
    pub fn openai(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("openai", api_key, api_base, Some(default_model.into()))
    }

    /// Create an OpenRouter provider
    pub fn openrouter(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("openrouter", api_key, api_base, Some(default_model.into()))
    }

    /// Create an Anthropic provider (via OpenAI-compatible endpoint)
    pub fn anthropic(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("anthropic", api_key, api_base, Some(default_model.into()))
    }

    /// Create a DashScope (阿里云通义千问) provider
    pub fn dashscope(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("dashscope", api_key, api_base, Some(default_model.into()))
    }

    /// Create a Moonshot AI (Kimi) provider
    pub fn moonshot(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("moonshot", api_key, api_base, Some(default_model.into()))
    }

    /// Create a Zhipu AI (智谱) provider
    pub fn zhipu(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("zhipu", api_key, api_base, Some(default_model.into()))
    }

    /// Create a MiniMax provider
    pub fn minimax(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
        group_id: Option<String>,
    ) -> Self {
        let mut extra_headers = HashMap::new();
        if let Some(gid) = group_id {
            extra_headers.insert("X-Group-Id".to_string(), gid);
        }
        Self::from_name_with_headers("minimax", api_key, api_base, Some(default_model.into()), extra_headers)
    }

    /// Create a DeepSeek provider (OpenAI-compatible, supports `reasoning_content`)
    pub fn deepseek(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("deepseek", api_key, api_base, Some(default_model.into()))
    }

    /// Create an Ollama provider (local LLM server with OpenAI-compatible API)
    ///
    /// Ollama runs locally and provides an OpenAI-compatible API endpoint.
    /// Default endpoint is `http://localhost:11434/v1`.
    /// Since Ollama is a local service, no API key is required (uses placeholder).
    pub fn ollama(
        api_base: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self::from_name("ollama", "ollama", api_base, Some(default_model.into()))
    }

    /// Get the provider name
    pub fn provider_name(&self) -> &str {
        &self.config.name
    }

    /// Get the API base URL
    pub fn api_base(&self) -> &str {
        &self.config.api_base
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    #[instrument(skip(self, request), fields(provider = %self.name(), model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.config.api_base);

        let openai_request = OpenAICompatibleRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        debug!(
            "[{}] POST {} | request body:\n{}",
            self.config.name,
            url,
            serde_json::to_string_pretty(&openai_request)
                .unwrap_or_else(|e| format!("<failed to serialize request: {}>", e))
        );

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json");

        // Apply extra headers (e.g. X-Group-Id for MiniMax)
        for (key, value) in &self.config.extra_headers {
            req = req.header(key, value);
        }

        let response = req.json(&openai_request).send().await?;

        let status = response.status();
        debug!("[{}] response status: {}", self.config.name, status);

        let body = response.text().await?;
        debug!("[{}] response body:\n{}", self.config.name, body);

        if !status.is_success() {
            anyhow::bail!("{} API error: {} - {}", self.config.name, status, body);
        }

        let api_response: OpenAICompatibleResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!(
                "{} API response parse error: {} | body: {}",
                self.config.name,
                e,
                body
            )
        })?;

        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in {} response", self.config.name))?;

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

        Ok(ChatResponse {
            content: choice.message.content,
            tool_calls,
            reasoning_content: choice.message.reasoning_content,
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
    /// DeepSeek R1 models return chain-of-thought here
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
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
            name: "test".to_string(),
            api_base: "https://api.example.com/v1".to_string(),
            api_key: "test-key".to_string(),
            default_model: "test-model".to_string(),
            extra_headers: HashMap::new(),
        };

        assert_eq!(config.api_base, "https://api.example.com/v1");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.default_model, "test-model");
    }

    #[test]
    fn test_openai_provider() {
        let provider = OpenAICompatibleProvider::openai("test-key", None, "gpt-4o");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.api_base(), "https://api.openai.com/v1");
    }

    #[test]
    fn test_openrouter_provider() {
        let provider =
            OpenAICompatibleProvider::openrouter("sk-or-test", None, "anthropic/claude-sonnet-4");
        assert_eq!(provider.name(), "openrouter");
        assert_eq!(provider.api_base(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn test_anthropic_provider() {
        let provider =
            OpenAICompatibleProvider::anthropic("sk-ant-test", None, "claude-sonnet-4-20250514");
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.api_base(), "https://api.anthropic.com/v1");
    }

    #[test]
    fn test_dashscope_provider() {
        let provider = OpenAICompatibleProvider::dashscope("test-key", None, "qwen-max");
        assert_eq!(provider.name(), "dashscope");
        assert_eq!(provider.default_model(), "qwen-max");
        assert_eq!(
            provider.api_base(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
    }

    #[test]
    fn test_moonshot_provider() {
        let provider = OpenAICompatibleProvider::moonshot("test-key", None, "moonshot-v1-8k");
        assert_eq!(provider.name(), "moonshot");
        assert_eq!(provider.api_base(), "https://api.moonshot.cn/v1");
    }

    #[test]
    fn test_zhipu_provider() {
        let provider = OpenAICompatibleProvider::zhipu("test-jwt", None, "GLM-5");
        assert_eq!(provider.name(), "zhipu");
        assert_eq!(provider.default_model(), "GLM-5");
        assert_eq!(provider.api_base(), "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn test_minimax_provider() {
        let provider = OpenAICompatibleProvider::minimax(
            "test-key",
            None,
            "abab6.5-chat",
            Some("group123".to_string()),
        );
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), "abab6.5-chat");
    }

    #[test]
    fn test_ollama_provider() {
        let provider = OpenAICompatibleProvider::ollama(None, "llama2");
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "llama2");
        assert_eq!(provider.api_base(), "http://localhost:11434/v1");
    }

    #[test]
    fn test_ollama_provider_custom_base() {
        let provider =
            OpenAICompatibleProvider::ollama(Some("http://192.168.1.100:11434/v1".to_string()), "mistral");
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "mistral");
        assert_eq!(provider.api_base(), "http://192.168.1.100:11434/v1");
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
