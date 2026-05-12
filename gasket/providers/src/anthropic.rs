//! Anthropic Claude LLM provider
//!
//! Uses rig's Anthropic client for API communication.

use crate::base::ChatStream;
use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{ChatRequest, ChatResponse, LlmProvider, ProviderError};
use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use std::collections::HashMap;
use tracing::{debug, instrument};

/// Default model for Anthropic
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// Default max tokens for Anthropic (required parameter)
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic provider using rig's client
pub struct AnthropicProvider {
    /// Rig Anthropic client
    rig_client: rig::providers::anthropic::Client<crate::logging_http::LoggingHttpClient>,

    /// Default model
    default_model: String,

    /// Default max tokens
    default_max_tokens: u32,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(api_key: String) -> Self {
        let rig_client = rig::providers::anthropic::Client::builder()
            .api_key(api_key)
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Anthropic client");
        Self {
            rig_client,
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Create with proxy configuration
    pub fn with_proxy(
        api_key: String,
        proxy_url: Option<String>,
        _proxy_username: Option<String>,
        _proxy_password: Option<String>,
    ) -> Self {
        let mut builder = rig::providers::anthropic::Client::builder().api_key(api_key);
        if let Some(url) = proxy_url {
            builder = builder.base_url(&url);
        }
        let rig_client = builder
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Anthropic client");
        Self {
            rig_client,
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        let rig_client = rig::providers::anthropic::Client::builder()
            .api_key(api_key)
            .base_url(&api_base)
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Anthropic client");
        Self {
            rig_client,
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Create with full configuration
    #[allow(clippy::too_many_arguments)]
    pub fn with_config(
        api_key: String,
        api_base: Option<String>,
        default_model: Option<String>,
        default_max_tokens: Option<u32>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        let http = crate::common::build_http_client(
            proxy_url.as_deref(),
            proxy_username.as_deref(),
            proxy_password.as_deref(),
        );
        let mut builder = rig::providers::anthropic::Client::builder()
            .api_key(api_key)
            .http_client(crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers));
        if let Some(base) = api_base {
            builder = builder.base_url(&base);
        }
        let rig_client = builder.build().expect("Failed to create Anthropic client");
        Self {
            rig_client,
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_max_tokens: default_max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Set default max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.default_max_tokens = max_tokens;
        self
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    #[instrument(skip(self), fields(provider = "anthropic", model = %model))]
    async fn model_limits(
        &self,
        model: &str,
    ) -> Result<Option<crate::ModelLimits>, crate::ProviderError> {
        // Rig's Anthropic client handles model listing internally
        // For now, return None - this is a less commonly used endpoint
        let _ = model;
        Ok(None)
    }

    #[instrument(skip(self, request), fields(provider = "anthropic", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        // Ensure max_tokens is set (Anthropic requires it)
        let mut request = request;
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.default_max_tokens);
        }

        let rig_request = to_rig_request(request);
        let rig_response = self
            .rig_client
            .completion_model(&model)
            .completion(rig_request)
            .await
            .map_err(|e| {
                debug!("[anthropic] rig error: {}", e);
                ProviderError::Other(e.to_string())
            })?;

        Ok(from_rig_response(rig_response))
    }

    #[instrument(skip(self, request), fields(provider = "anthropic", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        // Ensure max_tokens is set (Anthropic requires it)
        let mut request = request;
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.default_max_tokens);
        }

        let rig_request = to_rig_request(request);
        let stream = self
            .rig_client
            .completion_model(&model)
            .stream(rig_request)
            .await
            .map_err(|e| {
                debug!("[anthropic] rig stream error: {}", e);
                ProviderError::Other(e.to_string())
            })?;

        Ok(from_rig_stream(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = AnthropicProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_custom_model() {
        let provider = AnthropicProvider::new("test-key".to_string())
            .with_model("claude-opus-4".to_string());
        assert_eq!(provider.default_model(), "claude-opus-4");
    }
}