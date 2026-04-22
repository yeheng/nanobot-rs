//! Common provider functionality for OpenAI-compatible APIs
//!
//! This module provides a generic, reusable provider implementation for any
//! LLM service that speaks the OpenAI-compatible API format.
//!
//! # Usage
//!
//! All providers require explicit `api_base` configuration. No implicit
//! defaults are provided - you must specify the API endpoint.
//!
//! ```ignore
//! use gasket_providers::{OpenAICompatibleProvider, ProviderConfig};
//! use std::collections::HashMap;
//!
//! // Using the constructor directly
//! let config = ProviderConfig {
//!     api_base: "https://api.example.com/v1".to_string(),
//!     api_key: Some("your-api-key".to_string()),
//!     default_model: "model-id".to_string(),
//!     extra_headers: HashMap::new(),
//!     proxy_url: None,
//!     proxy_username: None,
//!     proxy_password: None,
//!     ..Default::default()
//! };
//! let provider = OpenAICompatibleProvider::new("my-provider", config);
//!
//! // Using from_name helper
//! let provider = OpenAICompatibleProvider::from_name(
//!     "openai",
//!     "your-api-key",
//!     "https://api.openai.com/v1".to_string(),
//!     Some("gpt-4o".to_string()),
//!     None,
//!     None,
//!     None,
//! );
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, instrument};

/// Errors that can occur when creating or using a provider.
#[derive(Debug, Error)]
pub enum ProviderBuildError {
    /// The provider is missing the required api_base configuration.
    #[error("Provider '{name}' is missing required 'api_base' configuration")]
    MissingApiBase {
        /// The provider name that was missing api_base
        name: String,
    },
}

/// Result type for provider operations.
pub type ProviderResult<T> = Result<T, ProviderBuildError>;

use crate::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ThinkingConfig, ToolCall,
    ToolDefinition,
};

/// Build an HTTP client with optional proxy support.
///
/// # Arguments
/// * `proxy_url` - Optional proxy URL (e.g., `http://127.0.0.1:7890`).
///   If provided, the client will use this proxy for all requests.
///   If `None`, all proxy settings are bypassed and environment variables are ignored.
/// * `proxy_username` - Optional username for proxy authentication.
/// * `proxy_password` - Optional password for proxy authentication.
pub fn build_http_client(
    proxy_url: Option<&str>,
    proxy_username: Option<&str>,
    proxy_password: Option<&str>,
) -> Client {
    let mut builder = Client::builder();

    if let Some(url) = proxy_url {
        let mut proxy = reqwest::Proxy::all(url).unwrap_or_else(|e| {
            tracing::warn!("Failed to create proxy for {}: {}", url, e);
            reqwest::Proxy::custom(|_| None::<&str>)
        });
        if let (Some(user), Some(pass)) = (proxy_username, proxy_password) {
            proxy = proxy.basic_auth(user, pass);
        }
        builder = builder.proxy(proxy);
        info!("HTTP client created with proxy: {}", url);
    } else {
        // Disable all proxies explicitly and ignore environment variables
        builder = builder.no_proxy();
        info!("HTTP client created with proxy disabled");
    }

    builder.build().unwrap_or_else(|e| {
        tracing::warn!(
            "Failed to build HTTP client with custom settings: {}, using default",
            e
        );
        Client::new()
    })
}

/// Provider API protocol type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    #[default]
    Openai,
    Anthropic,
    Gemini,
    Moonshot,
    Minimax,
}

/// Model-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    #[serde(default, alias = "priceInputPerMillion")]
    pub price_input_per_million: Option<f64>,
    #[serde(default, alias = "priceOutputPerMillion")]
    pub price_output_per_million: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: Option<u32>,
    #[serde(default, alias = "maxIterations")]
    pub max_iterations: Option<u32>,
    #[serde(default, alias = "memoryWindow")]
    pub memory_window: Option<usize>,
    #[serde(default, alias = "thinkingEnabled")]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
}

/// Common provider configuration
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    #[serde(default)]
    /// API base URL (e.g. "https://api.openai.com/v1") - REQUIRED
    pub api_base: String,
    #[serde(default)]
    /// API key
    pub api_key: Option<String>,
    #[serde(default)]
    /// Default model
    pub default_model: String,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    /// Extra HTTP headers to send with every request
    pub extra_headers: HashMap<String, String>,
    #[serde(default, alias = "proxyUrl")]
    /// Optional proxy URL (e.g., `http://127.0.0.1:7890`)
    pub proxy_url: Option<String>,
    #[serde(default, alias = "proxyUsername")]
    /// Optional username for proxy authentication
    pub proxy_username: Option<String>,
    #[serde(default, alias = "proxyPassword")]
    /// Optional password for proxy authentication
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default, alias = "defaultCurrency")]
    pub default_currency: Option<String>,
    #[serde(default, alias = "supportsThinking")]
    pub supports_thinking: bool,
}

impl ProviderConfig {
    pub fn is_available(&self, _name: &str) -> bool {
        self.api_key.is_some()
            || self.api_base.contains("localhost")
            || self.api_base.contains("127.0.0.1")
    }

    pub fn thinking_enabled_for_model(&self, model: &str) -> bool {
        self.models
            .get(model)
            .and_then(|m| m.thinking_enabled)
            .unwrap_or(false)
    }

    pub fn get_pricing_for_model(
        &self,
        model: &str,
    ) -> Option<gasket_types::token_tracker::ModelPricing> {
        self.models.get(model).and_then(|m| {
            match (m.price_input_per_million, m.price_output_per_million) {
                (Some(input), Some(output)) => Some(gasket_types::token_tracker::ModelPricing {
                    price_input_per_million: input,
                    price_output_per_million: output,
                    currency: m.currency.clone().unwrap_or_else(|| "USD".to_string()),
                }),
                _ => None,
            }
        })
    }
}

/// OpenAI-compatible provider that implements `LlmProvider`.
///
/// This single struct replaces per-vendor provider files (dashscope.rs,
/// moonshot.rs, zhipu.rs, minimax.rs, etc.) — all of which performed
/// identical HTTP POST + JSON parse logic.
pub struct OpenAICompatibleProvider {
    name: String,
    client: Client,
    config: ProviderConfig,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    pub fn new(name: impl Into<String>, config: ProviderConfig) -> Self {
        let client = build_http_client(
            config.proxy_url.as_deref(),
            config.proxy_username.as_deref(),
            config.proxy_password.as_deref(),
        );
        Self {
            name: name.into(),
            client,
            config,
        }
    }

    /// Create with custom HTTP client
    pub fn with_client(name: impl Into<String>, config: ProviderConfig, client: Client) -> Self {
        Self {
            name: name.into(),
            client,
            config,
        }
    }

    /// Create a provider by name with explicit configuration.
    ///
    /// All providers require explicit `api_base` configuration.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider display name
    /// * `api_key` - API key for authentication
    /// * `api_base` - **Required** API base URL (e.g., "https://api.openai.com/v1")
    /// * `default_model` - Optional default model ID (defaults to "default")
    /// * `proxy_url` - Optional proxy URL (e.g., `http://127.0.0.1:7890`)
    /// * `proxy_username` - Optional username for proxy authentication
    /// * `proxy_password` - Optional password for proxy authentication
    ///
    /// # Example
    /// ```ignore
    /// let provider = OpenAICompatibleProvider::from_name(
    ///     "openai",
    ///     "your-api-key",
    ///     "https://api.openai.com/v1".to_string(),
    ///     Some("gpt-4o".to_string()),
    ///     None,
    ///     None,
    ///     None,
    /// );
    /// ```
    pub fn from_name(
        name: &str,
        api_key: impl Into<String>,
        api_base: String,
        default_model: Option<String>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        let resolved_model = default_model.unwrap_or_else(|| "default".to_string());

        Self::new(
            name,
            ProviderConfig {
                provider_type: ProviderType::Openai,
                api_base,
                api_key: Some(api_key.into()),
                default_model: resolved_model,
                models: HashMap::new(),
                extra_headers: HashMap::new(),
                proxy_url,
                proxy_username,
                proxy_password,
                client_id: None,
                default_currency: None,
                supports_thinking: false,
            },
        )
    }

    /// Create a provider by name with extra headers (e.g., for MiniMax's X-Group-Id)
    #[allow(clippy::too_many_arguments)]
    pub fn from_name_with_headers(
        name: &str,
        api_key: impl Into<String>,
        api_base: String,
        default_model: Option<String>,
        extra_headers: HashMap<String, String>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        let mut provider = Self::from_name(
            name,
            api_key,
            api_base,
            default_model,
            proxy_url,
            proxy_username,
            proxy_password,
        );
        provider.config.extra_headers = extra_headers;
        provider
    }

    // -- Special constructors --

    /// Create a MiniMax provider
    pub fn minimax(
        api_key: impl Into<String>,
        api_base: String,
        default_model: impl Into<String>,
        group_id: Option<String>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
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
            proxy_url,
            proxy_username,
            proxy_password,
        )
    }

    /// Get the provider name
    pub fn provider_name(&self) -> &str {
        &self.name
    }

    /// Get the API base URL
    pub fn api_base(&self) -> &str {
        &self.config.api_base
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    #[instrument(skip(self, request), fields(provider = %self.name(), model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
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

        info!("[{}] POST {} ", self.name, url);

        let mut req = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.api_key.as_deref().unwrap_or("")),
            )
            .header("Content-Type", "application/json");

        // Apply extra headers (e.g. X-Group-Id for MiniMax)
        for (key, value) in &self.config.extra_headers {
            req = req.header(key, value);
        }

        let response =
            req.json(&openai_request).send().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!("Request failed: {}", e))
            })?;

        let status = response.status();
        info!("[{}] response status: {}", self.name, status);

        let body = response.text().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Failed to read response: {}", e))
        })?;

        if !status.is_success() {
            error!("[{}] response body:\n{}", self.name, body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, body),
            });
        }

        let api_response: OpenAICompatibleResponse = serde_json::from_str(&body).map_err(|e| {
            crate::ProviderError::ParseError(format!(
                "{} API response parse error: {} | body: {}",
                self.name, e, body
            ))
        })?;

        let choice = api_response.choices.into_iter().next().ok_or_else(|| {
            crate::ProviderError::ParseError(format!("No choices in {} response", self.name))
        })?;

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
        let usage = api_response.usage.map(|u| crate::Usage {
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
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
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

        info!("[{}] POST {}", self.name, url);

        let mut req = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.api_key.as_deref().unwrap_or("")),
            )
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.extra_headers {
            req = req.header(key, value);
        }

        let response =
            req.json(&openai_request).send().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!("Request failed: {}", e))
            })?;

        let status = response.status();
        info!("[{}] stream response status: {}", self.name, status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!("Failed to read error body: {}", e))
            })?;

            error!("[{}] POST {} response: {}", self.name, url, body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = crate::streaming::parse_sse_stream(byte_stream);

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
            provider_type: ProviderType::Openai,
            api_base: "https://api.example.com/v1".to_string(),
            api_key: Some("test-key".to_string()),
            default_model: "test-model".to_string(),
            models: HashMap::new(),
            extra_headers: HashMap::new(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            client_id: None,
            default_currency: None,
            supports_thinking: false,
        };

        assert_eq!(config.api_base, "https://api.example.com/v1");
        assert_eq!(config.api_key, Some("test-key".to_string()));
        assert_eq!(config.default_model, "test-model");
        assert!(config.proxy_url.is_none());
        assert!(config.proxy_username.is_none());
        assert!(config.proxy_password.is_none());
        assert!(!config.supports_thinking);
    }

    #[test]
    fn test_openai_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "openai",
            "test-key",
            "https://api.openai.com/v1".to_string(),
            Some("gpt-4o".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.api_base(), "https://api.openai.com/v1");
    }

    #[test]
    fn test_openrouter_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "openrouter",
            "sk-or-test",
            "https://openrouter.ai/api/v1".to_string(),
            Some("anthropic/claude-sonnet-4".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "openrouter");
        assert_eq!(provider.api_base(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn test_anthropic_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "anthropic",
            "sk-ant-test",
            "https://api.anthropic.com/v1".to_string(),
            Some("claude-sonnet-4-20250514".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.api_base(), "https://api.anthropic.com/v1");
    }

    #[test]
    fn test_dashscope_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "dashscope",
            "test-key",
            "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            Some("qwen-max".to_string()),
            None,
            None,
            None,
        );
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
            "https://api.moonshot.cn/v1".to_string(),
            Some("moonshot-v1-8k".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "moonshot");
        assert_eq!(provider.api_base(), "https://api.moonshot.cn/v1");
    }

    #[test]
    fn test_zhipu_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "zhipu",
            "test-jwt",
            "https://open.bigmodel.cn/api/paas/v4".to_string(),
            Some("GLM-5".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "zhipu");
        assert_eq!(provider.default_model(), "GLM-5");
        assert_eq!(provider.api_base(), "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn test_minimax_provider() {
        let provider = OpenAICompatibleProvider::minimax(
            "test-key",
            "https://api.minimax.chat/v1".to_string(),
            "abab6.5-chat",
            Some("group123".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), "abab6.5-chat");
    }

    #[test]
    fn test_ollama_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "ollama",
            "ollama",
            "http://localhost:11434/v1".to_string(),
            Some("llama2".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "llama2");
        assert_eq!(provider.api_base(), "http://localhost:11434/v1");
    }

    #[test]
    fn test_ollama_provider_custom_base() {
        let provider = OpenAICompatibleProvider::from_name(
            "ollama",
            "ollama",
            "http://192.168.1.100:11434/v1".to_string(),
            Some("mistral".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.default_model(), "mistral");
        assert_eq!(provider.api_base(), "http://192.168.1.100:11434/v1");
    }

    #[test]
    fn test_litellm_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "litellm",
            "", // LiteLLM may not require API key
            "http://localhost:4000/v1".to_string(),
            Some("gpt-4o".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "litellm");
        assert_eq!(provider.default_model(), "gpt-4o");
        assert_eq!(provider.api_base(), "http://localhost:4000/v1");
    }

    #[test]
    fn test_litellm_provider_custom_base() {
        let provider = OpenAICompatibleProvider::from_name(
            "litellm",
            "sk-test-key",
            "http://192.168.1.100:4000/v1".to_string(),
            Some("claude-3-opus".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(provider.name(), "litellm");
        assert_eq!(provider.default_model(), "claude-3-opus");
        assert_eq!(provider.api_base(), "http://192.168.1.100:4000/v1");
    }

    #[test]
    fn test_custom_provider() {
        let provider = OpenAICompatibleProvider::from_name(
            "custom-provider",
            "test-key",
            "https://custom.api.com/v1".to_string(),
            Some("custom-model".to_string()),
            None,
            None,
            None,
        );
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
