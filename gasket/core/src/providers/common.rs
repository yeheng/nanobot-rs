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
use thiserror::Error;
use tracing::{info, instrument};

/// Errors that can occur when creating or using a provider.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The provider name is unknown and no api_base was provided.
    #[error("Unknown provider '{name}'. Add it to PROVIDER_DEFAULTS or provide api_base.")]
    UnknownProvider {
        /// The provider name that was not recognized
        name: String,
    },
}

/// Result type for provider operations.
pub type ProviderResult<T> = Result<T, ProviderError>;

use super::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ThinkingConfig, ToolCall,
    ToolDefinition,
};

/// Default configuration for known providers
///
/// This is data, not code. Adding a new provider just means adding a row here.
static PROVIDER_DEFAULTS: &[(&str, &str, Option<&str>)] = &[
    // (name, api_base, default_model)
    ("openai", "https://api.openai.com/v1", Some("gpt-4o")),
    (
        "openrouter",
        "https://openrouter.ai/api/v1",
        Some("anthropic/claude-sonnet-4"),
    ),
    (
        "anthropic",
        "https://api.anthropic.com/v1",
        Some("claude-sonnet-4-20250514"),
    ),
    (
        "dashscope",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        Some("qwen-max"),
    ),
    ("moonshot", "https://api.moonshot.cn/v1", Some("kimi-k2.5")),
    (
        "zhipu",
        "https://open.bigmodel.cn/api/paas/v4",
        Some("glm-5"),
    ),
    (
        "zhipu_coding",
        "https://open.bigmodel.cn/api/coding/paas/v4",
        Some("glm-5"),
    ),
    ("minimax", "https://api.minimax.chat/v1", Some("M2.2")),
    (
        "deepseek",
        "https://api.deepseek.com/v1",
        Some("deepseek-chat"),
    ),
    // Local providers (no API key required by default)
    ("ollama", "http://localhost:11434/v1", Some("llama3")),
    ("litellm", "http://localhost:4000/v1", Some("gpt-4o")),
];

/// Get default API base URL for a provider name
pub fn get_default_api_base(name: &str) -> Option<&'static str> {
    PROVIDER_DEFAULTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, url, _)| *url)
}

/// Get default model for a provider name
pub fn get_default_model(name: &str) -> Option<&'static str> {
    PROVIDER_DEFAULTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .and_then(|(_, _, model)| *model)
}

/// Build an HTTP client with optional proxy support.
///
/// # Arguments
/// * `proxy_enabled` - If `true`, the client will use proxy settings from
///   environment variables (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY, NO_PROXY).
///   If `false`, all proxy settings are bypassed.
///
/// # Environment Variables (when proxy is enabled)
/// - `HTTP_PROXY` / `http_proxy`: Proxy for HTTP requests
/// - `HTTPS_PROXY` / `https_proxy`: Proxy for HTTPS requests
/// - `ALL_PROXY` / `all_proxy`: Proxy for all requests
/// - `NO_PROXY` / `no_proxy`: Hosts to bypass proxy
pub fn build_http_client(proxy_enabled: bool) -> Client {
    let mut builder = Client::builder();

    if !proxy_enabled {
        // Disable all proxies explicitly
        builder = builder.no_proxy();
        info!("HTTP client created with proxy disabled");
    } else {
        // Default behavior: reqwest automatically reads environment variables
        info!("HTTP client created with proxy enabled (using environment variables)");
    }

    builder.build().unwrap_or_else(|e| {
        tracing::warn!(
            "Failed to build HTTP client with custom settings: {}, using default",
            e
        );
        Client::new()
    })
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
    /// Whether to enable HTTP proxy (default: true)
    pub proxy_enabled: bool,
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
        let client = build_http_client(config.proxy_enabled);
        Self { client, config }
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
    /// # Errors
    ///
    /// Returns `ProviderError::UnknownProvider` if the provider name is not in
    /// `PROVIDER_DEFAULTS` and no `api_base` is provided.
    ///
    /// # Example
    /// ```ignore
    /// let provider = OpenAICompatibleProvider::from_name("dashscope", "your-api-key", None, None, true)?;
    /// ```
    pub fn from_name(
        name: &str,
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
        proxy_enabled: bool,
    ) -> ProviderResult<Self> {
        let resolved_base = api_base
            .or_else(|| get_default_api_base(name).map(|s| s.to_string()))
            .ok_or_else(|| ProviderError::UnknownProvider {
                name: name.to_string(),
            })?;

        let resolved_model = default_model
            .or_else(|| get_default_model(name).map(|s| s.to_string()))
            .unwrap_or_else(|| "default".to_string());

        Ok(Self::new(ProviderConfig {
            name: name.to_string(),
            api_base: resolved_base,
            api_key: api_key.into(),
            default_model: resolved_model,
            extra_headers: HashMap::new(),
            proxy_enabled,
        }))
    }

    /// Create a provider by name with extra headers (e.g., for MiniMax's X-Group-Id)
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::UnknownProvider` if the provider name is not in
    /// `PROVIDER_DEFAULTS` and no `api_base` is provided.
    pub fn from_name_with_headers(
        name: &str,
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
        extra_headers: HashMap<String, String>,
        proxy_enabled: bool,
    ) -> ProviderResult<Self> {
        let mut provider = Self::from_name(name, api_key, api_base, default_model, proxy_enabled)?;
        provider.config.extra_headers = extra_headers;
        Ok(provider)
    }

    // -- Special constructors --

    /// Create a MiniMax provider
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::UnknownProvider` if MiniMax is not in
    /// `PROVIDER_DEFAULTS` (which should never happen).
    pub fn minimax(
        api_key: impl Into<String>,
        api_base: Option<String>,
        default_model: impl Into<String>,
        group_id: Option<String>,
        proxy_enabled: bool,
    ) -> ProviderResult<Self> {
        let mut extra_headers = HashMap::new();
        if let Some(gid) = group_id {
            extra_headers.insert("X-Group-Id".to_string(), gid);
        }
        Self::from_name_with_headers(
            "minimax",
            api_key,
            api_base,
            Some(default_model.into()),
            extra_headers,
            proxy_enabled,
        )
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
            thinking: request.thinking,
            stream: false,
        };

        tracing::trace!(
            "[{}] POST {} | request body:\n{}",
            self.config.name,
            url,
            serde_json::to_string(&openai_request)
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
        info!("[{}] response status: {}", self.config.name, status);

        let body = response.text().await?;
        info!("[{}] response body:\n{}", self.config.name, body);

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

        // Convert API usage to ChatResponse usage
        let usage = api_response.usage.map(|u| crate::providers::Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatResponse {
            content: choice.message.content,
            tool_calls,
            reasoning_content: choice.message.reasoning_content,
            usage,
        })
    }

    #[instrument(skip(self, request), fields(provider = %self.name(), model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> anyhow::Result<ChatStream> {
        let url = format!("{}/chat/completions", self.config.api_base);

        let openai_request = OpenAICompatibleRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            thinking: request.thinking,
            stream: true,
        };

        tracing::trace!(
            "[{}] POST {} (stream) | request body:\n{}",
            self.config.name,
            url,
            serde_json::to_string(&openai_request)
                .unwrap_or_else(|e| format!("<failed to serialize request: {}>", e))
        );

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.extra_headers {
            req = req.header(key, value);
        }

        let response = req.json(&openai_request).send().await?;

        let status = response.status();
        info!("[{}] stream response status: {}", self.config.name, status);

        if !status.is_success() {
            let body = response.text().await?;
            anyhow::bail!("{} API error: {} - {}", self.config.name, status, body);
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = super::streaming::parse_sse_stream(byte_stream);

        Ok(Box::pin(chunk_stream))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleResponse {
    choices: Vec<OpenAICompatibleChoice>,
    #[serde(default)]
    usage: Option<OpenAICompatibleUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompatibleUsage {
    #[serde(default, rename = "prompt_tokens")]
    input_tokens: usize,
    #[serde(default, rename = "completion_tokens")]
    output_tokens: usize,
    #[serde(default, rename = "total_tokens")]
    total_tokens: usize,
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
            proxy_enabled: true,
        };

        assert_eq!(config.api_base, "https://api.example.com/v1");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.default_model, "test-model");
        assert!(config.proxy_enabled);
    }

    #[test]
    fn test_openai_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "openai",
            "test-key",
            None,
            Some("gpt-4o".to_string()),
            true,
        )
        .expect("openai should be known provider");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.api_base(), "https://api.openai.com/v1");
    }

    #[test]
    fn test_openrouter_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "openrouter",
            "sk-or-test",
            None,
            Some("anthropic/claude-sonnet-4".to_string()),
            true,
        )
        .expect("openrouter should be known provider");
        assert_eq!(provider.name(), "openrouter");
        assert_eq!(provider.api_base(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn test_anthropic_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "anthropic",
            "sk-ant-test",
            None,
            Some("claude-sonnet-4-20250514".to_string()),
            true,
        )
        .expect("anthropic should be known provider");
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.api_base(), "https://api.anthropic.com/v1");
    }

    #[test]
    fn test_dashscope_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "dashscope",
            "test-key",
            None,
            Some("qwen-max".to_string()),
            true,
        )
        .expect("dashscope should be known provider");
        assert_eq!(provider.name(), "dashscope");
        assert_eq!(provider.default_model(), "qwen-max");
        assert_eq!(
            provider.api_base(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
    }

    #[test]
    fn test_moonshot_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "moonshot",
            "test-key",
            None,
            Some("moonshot-v1-8k".to_string()),
            true,
        )
        .expect("moonshot should be known provider");
        assert_eq!(provider.name(), "moonshot");
        assert_eq!(provider.api_base(), "https://api.moonshot.cn/v1");
    }

    #[test]
    fn test_zhipu_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "zhipu",
            "test-jwt",
            None,
            Some("GLM-5".to_string()),
            true,
        )
        .expect("zhipu should be known provider");
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
            true,
        )
        .expect("minimax should be known provider");
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), "abab6.5-chat");
    }

    #[test]
    fn test_ollama_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "ollama",
            "ollama",
            None,
            Some("llama2".to_string()),
            true,
        )
        .expect("ollama should be known provider");
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "llama2");
        assert_eq!(provider.api_base(), "http://localhost:11434/v1");
    }

    #[test]
    fn test_ollama_provider_custom_base() {
        let provider = OpenAICompatibleProvider::from_name(
            "ollama",
            "ollama",
            Some("http://192.168.1.100:11434/v1".to_string()),
            Some("mistral".to_string()),
            true,
        )
        .expect("ollama should be known provider");
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "mistral");
        assert_eq!(provider.api_base(), "http://192.168.1.100:11434/v1");
    }

    #[test]
    fn test_litellm_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "litellm",
            "", // LiteLLM may not require API key
            None,
            Some("gpt-4o".to_string()),
            true,
        )
        .expect("litellm should be known provider");
        assert_eq!(provider.name(), "litellm");
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.api_base(), "http://localhost:4000/v1");
    }

    #[test]
    fn test_litellm_provider_custom_base() {
        let provider = OpenAICompatibleProvider::from_name(
            "litellm",
            "sk-test-key",
            Some("http://192.168.1.100:4000/v1".to_string()),
            Some("claude-3-opus".to_string()),
            true,
        )
        .expect("litellm should be known provider");
        assert_eq!(provider.name(), "litellm");
        assert_eq!(provider.default_model(), "claude-3-opus");
        assert_eq!(provider.api_base(), "http://192.168.1.100:4000/v1");
    }

    #[test]
    fn test_unknown_provider_error() {
        let result = OpenAICompatibleProvider::from_name(
            "unknown-provider",
            "test-key",
            None, // No api_base provided
            None,
            true,
        );
        assert!(result.is_err());
        match result {
            Err(ProviderError::UnknownProvider { name }) => {
                assert_eq!(name, "unknown-provider");
            }
            _ => panic!("Expected UnknownProvider error"),
        }
    }

    #[test]
    fn test_unknown_provider_with_custom_base() {
        // Unknown provider with custom api_base should succeed
        let provider = OpenAICompatibleProvider::from_name(
            "custom-provider",
            "test-key",
            Some("https://custom.api.com/v1".to_string()),
            Some("custom-model".to_string()),
            true,
        )
        .expect("unknown provider with api_base should succeed");
        assert_eq!(provider.name(), "custom-provider");
        assert_eq!(provider.api_base(), "https://custom.api.com/v1");
        assert_eq!(provider.default_model(), "custom-model");
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
