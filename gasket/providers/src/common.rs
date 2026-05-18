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
use tracing::{info, instrument};

use rig::providers::openai;

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
    ChatRequest, ChatResponse, ChatStream, LlmProvider, RigCompletionProvider,
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
    api_base: String,
    inner: RigCompletionProvider<openai::CompletionsClient<crate::logging_http::LoggingHttpClient>>,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    pub fn new(name: impl Into<String>, config: ProviderConfig) -> Self {
        let api_key = config.api_key.clone().unwrap_or_default();
        let supports_thinking = config.supports_thinking;

        let http_client = build_http_client(
            config.proxy_url.as_deref(),
            config.proxy_username.as_deref(),
            config.proxy_password.as_deref(),
        );

        let logging_client = crate::logging_http::LoggingHttpClient::new(http_client.clone())
            .with_extra_headers(config.extra_headers.clone());

        let rig_client = if config.api_base.is_empty() {
            openai::CompletionsClient::builder()
                .api_key(api_key)
                .http_client(logging_client.clone())
                .build()
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to create rig client: {}", e);
                    openai::CompletionsClient::builder()
                        .api_key("")
                        .http_client(logging_client.clone())
                        .build()
                        .expect("fallback rig client creation should not fail")
                })
        } else {
            openai::CompletionsClient::builder()
                .api_key(api_key.clone())
                .base_url(&config.api_base)
                .http_client(logging_client.clone())
                .build()
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to create rig client with base_url: {}", e);
                    openai::CompletionsClient::builder()
                        .api_key(api_key)
                        .http_client(logging_client.clone())
                        .build()
                        .unwrap_or_else(|e2| {
                            panic!("Fallback rig client creation also failed: {}", e2)
                        })
                })
        };
        let name = name.into();
        let inner = RigCompletionProvider::new(&name, config.default_model.clone(), rig_client)
            .with_thinking(supports_thinking);
        Self {
            api_base: config.api_base.clone(),
            name,
            inner,
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
        let resolved_model = default_model.unwrap_or_else(|| "default".to_string());
        Self::new(
            name,
            ProviderConfig {
                provider_type: ProviderType::Openai,
                api_base,
                api_key: Some(api_key.into()),
                default_model: resolved_model,
                models: HashMap::new(),
                extra_headers,
                proxy_url,
                proxy_username,
                proxy_password,
                client_id: None,
                default_currency: None,
                supports_thinking: false,
            },
        )
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
        &self.api_base
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn supports_thinking(&self) -> bool {
        self.inner.supports_thinking()
    }

    #[instrument(skip(self, request), fields(provider = %self.name(), model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        self.inner.chat(request).await
    }

    #[instrument(skip(self, request), fields(provider = %self.name(), model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        self.inner.chat_stream(request).await
    }
}

/// Build a provider instance by name and config.
///
/// Routes to the appropriate native provider when the name is recognized,
/// falling back to `provider_type` dispatch for unknown names.
pub fn build_provider(
    name: &str,
    api_key: &str,
    provider_config: &ProviderConfig,
    model: &str,
) -> anyhow::Result<std::sync::Arc<dyn crate::LlmProvider>> {
    let proxy_url = provider_config.proxy_url.clone();
    let proxy_username = provider_config.proxy_username.clone();
    let proxy_password = provider_config.proxy_password.clone();
    let extra_headers = provider_config.extra_headers.clone();
    let api_base = provider_config.api_base.clone();

    match name {
        "minimax" | "minimaxi" => {
            #[cfg(feature = "provider-minimax")]
            {
                let provider = crate::build_minimax_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-minimax"))]
            anyhow::bail!(
                "MiniMax provider is not compiled in. Rebuild with --features provider-minimax"
            )
        }
        "gemini" => {
            #[cfg(feature = "provider-gemini")]
            {
                let provider = crate::build_gemini_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-gemini"))]
            anyhow::bail!(
                "Gemini provider is not compiled in. Rebuild with --features provider-gemini"
            )
        }
        "moonshot" | "kimi" => {
            #[cfg(feature = "provider-moonshot")]
            {
                let provider = crate::MoonshotProvider::with_config(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-moonshot"))]
            anyhow::bail!(
                "Moonshot provider is not compiled in. Rebuild with --features provider-moonshot"
            )
        }
        "anthropic" | "claude" => {
            #[cfg(feature = "provider-anthropic")]
            {
                let provider = crate::build_anthropic_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    None,
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-anthropic"))]
            anyhow::bail!(
                "Anthropic provider is not compiled in. Rebuild with --features provider-anthropic"
            )
        }
        "copilot" => {
            #[cfg(feature = "provider-copilot")]
            {
                let provider = crate::CopilotProvider::with_proxy(
                    api_key,
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                )?;
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-copilot"))]
            anyhow::bail!(
                "Copilot provider is not compiled in. Rebuild with --features provider-copilot"
            )
        }
        _ => dispatch_by_type(name, api_key, provider_config, model),
    }
}

/// Fallback: dispatch by `provider_type` for unknown provider names.
fn dispatch_by_type(
    name: &str,
    api_key: &str,
    provider_config: &ProviderConfig,
    model: &str,
) -> anyhow::Result<std::sync::Arc<dyn crate::LlmProvider>> {
    let proxy_url = provider_config.proxy_url.clone();
    let proxy_username = provider_config.proxy_username.clone();
    let proxy_password = provider_config.proxy_password.clone();
    let extra_headers = provider_config.extra_headers.clone();
    let api_base = provider_config.api_base.clone();

    match provider_config.provider_type {
        ProviderType::Gemini => {
            #[cfg(feature = "provider-gemini")]
            {
                let provider = crate::build_gemini_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-gemini"))]
            anyhow::bail!(
                "Gemini provider is not compiled in. Rebuild with --features provider-gemini"
            )
        }
        ProviderType::Minimax => {
            #[cfg(feature = "provider-minimax")]
            {
                let provider = crate::build_minimax_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-minimax"))]
            anyhow::bail!(
                "MiniMax provider is not compiled in. Rebuild with --features provider-minimax"
            )
        }
        ProviderType::Moonshot => {
            #[cfg(feature = "provider-moonshot")]
            {
                let provider = crate::MoonshotProvider::with_config(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-moonshot"))]
            anyhow::bail!(
                "Moonshot provider is not compiled in. Rebuild with --features provider-moonshot"
            )
        }
        ProviderType::Anthropic => {
            #[cfg(feature = "provider-anthropic")]
            {
                let provider = crate::build_anthropic_provider(
                    api_key.to_string(),
                    Some(api_base),
                    Some(model.to_string()),
                    None,
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    extra_headers,
                );
                Ok(std::sync::Arc::new(provider))
            }
            #[cfg(not(feature = "provider-anthropic"))]
            anyhow::bail!(
                "Anthropic provider is not compiled in. Rebuild with --features provider-anthropic"
            )
        }
        ProviderType::Openai => {
            let supports_thinking = matches!(name, "deepseek" | "kimi" | "moonshot" | "zhipu");
            let config = ProviderConfig {
                provider_type: ProviderType::Openai,
                api_base: provider_config.api_base.clone(),
                api_key: Some(api_key.to_string()),
                default_model: model.to_string(),
                models: HashMap::new(),
                extra_headers,
                proxy_url,
                proxy_username,
                proxy_password,
                client_id: provider_config.client_id.clone(),
                default_currency: provider_config.default_currency.clone(),
                supports_thinking,
            };
            Ok(std::sync::Arc::new(OpenAICompatibleProvider::new(name, config)))
        }
    }
}

/// Parse JSON arguments from string
pub fn parse_json_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}))
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
