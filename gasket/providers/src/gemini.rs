//! Google Gemini LLM provider
//!
//! Uses rig's Gemini client for API communication.

use crate::base::ChatStream;
use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{ChatRequest, ChatResponse, LlmProvider, ProviderError};
use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use std::collections::HashMap;
use tracing::{debug, instrument};

/// Gemini provider using rig's client
pub struct GeminiProvider {
    /// Rig Gemini client
    rig_client: rig::providers::gemini::Client<crate::logging_http::LoggingHttpClient>,

    /// Default model
    default_model: String,
}

impl GeminiProvider {
    /// Create a new Gemini provider
    pub fn new(api_key: String) -> Self {
        let rig_client = rig::providers::gemini::Client::builder()
            .api_key(api_key.clone())
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Gemini client");
        Self {
            rig_client,
            default_model: "gemini-2.5-flash".to_string(),
        }
    }

    /// Create with proxy configuration
    pub fn with_proxy(
        api_key: String,
        proxy_url: Option<String>,
        _proxy_username: Option<String>,
        _proxy_password: Option<String>,
    ) -> Self {
        let mut builder = rig::providers::gemini::Client::builder().api_key(api_key.clone());
        if let Some(url) = proxy_url {
            builder = builder.base_url(&url);
        }
        Self {
            rig_client: builder
                .http_client(crate::logging_http::LoggingHttpClient::default())
                .build()
                .expect("Failed to create Gemini client"),
            default_model: "gemini-2.5-flash".to_string(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        let rig_client = rig::providers::gemini::Client::builder()
            .api_key(api_key.clone())
            .base_url(&api_base)
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Gemini client");
        Self {
            rig_client,
            default_model: "gemini-2.5-flash".to_string(),
        }
    }

    /// Create with full configuration
    pub fn with_config(
        api_key: String,
        api_base: Option<String>,
        default_model: Option<String>,
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
        let mut builder = rig::providers::gemini::Client::builder()
            .api_key(api_key.clone())
            .http_client(crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers));
        if let Some(ref base) = api_base {
            builder = builder.base_url(base);
        }
        Self {
            rig_client: builder.build().expect("Failed to create Gemini client"),
            default_model: default_model.unwrap_or_else(|| "gemini-2.5-flash".to_string()),
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request), fields(provider = "gemini", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let rig_request = to_rig_request(request);
        let rig_response = self
            .rig_client
            .completion_model(&model)
            .completion(rig_request)
            .await
            .map_err(|e| {
                debug!("[gemini] rig error: {}", e);
                ProviderError::Other(e.to_string())
            })?;

        Ok(from_rig_response(rig_response))
    }

    #[instrument(skip(self, request), fields(provider = "gemini", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let rig_request = to_rig_request(request);
        let stream = self
            .rig_client
            .completion_model(&model)
            .stream(rig_request)
            .await
            .map_err(|e| {
                debug!("[gemini] rig stream error: {}", e);
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
        let provider = GeminiProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.default_model(), "gemini-2.5-flash");
    }

    #[test]
    fn test_custom_model() {
        let provider = GeminiProvider::new("test-key".to_string())
            .with_model("gemini-pro".to_string());
        assert_eq!(provider.default_model(), "gemini-pro");
    }
}