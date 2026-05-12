//! GitHub Copilot LLM Provider
//!
//! Implements the `LlmProvider` trait for GitHub Copilot's chat API using rig.
//! Supports GitHub Access Token, API Key, and OAuth authentication.
//!
//! For OAuth, use `Client::from_env()` which handles device flow automatically.

use std::collections::HashMap;

use async_trait::async_trait;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::CompletionModel;
use tracing::{debug, instrument};

use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{
    ChatRequest, ChatResponse, ChatStream, LlmProvider, ProviderError,
};

/// Default model for Copilot
const DEFAULT_MODEL: &str = "gpt-4o";

/// GitHub Copilot provider using rig
pub struct CopilotProvider {
    /// Provider name
    name: String,
    /// Rig copilot client
    rig_client: rig::providers::copilot::Client,
    /// Default model
    default_model: String,
}

impl CopilotProvider {
    /// Create a new Copilot provider with GitHub Access Token authentication
    ///
    /// # Arguments
    /// * `github_token` - GitHub access token (PAT or OAuth token)
    /// * `api_base` - Optional custom API base URL
    /// * `default_model` - Default model to use (e.g., "gpt-4o")
    pub fn new(
        github_token: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
    ) -> Result<Self, ProviderError> {
        let mut builder = rig::providers::copilot::Client::builder();

        if let Some(base) = api_base {
            builder = builder.base_url(base);
        }

        let client = builder
            .github_access_token(github_token)
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Ok(Self {
            name: "copilot".to_string(),
            rig_client: client,
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Create a new Copilot provider with API Key authentication
    ///
    /// # Arguments
    /// * `api_key` - Copilot API key
    /// * `default_model` - Default model to use (e.g., "gpt-4o")
    pub fn with_api_key(
        api_key: impl Into<String>,
        default_model: Option<String>,
    ) -> Result<Self, ProviderError> {
        use rig::providers::copilot::CopilotAuth;

        let client = rig::providers::copilot::Client::builder()
            .api_key(CopilotAuth::ApiKey(api_key.into()))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Ok(Self {
            name: "copilot".to_string(),
            rig_client: client,
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Create a new Copilot provider from environment variables
    ///
    /// This method checks for:
    /// 1. `GITHUB_COPILOT_API_KEY` or `COPILOT_API_KEY` - API key
    /// 2. `COPILOT_GITHUB_ACCESS_TOKEN` or `GITHUB_TOKEN` - GitHub access token
    /// 3. Falls back to OAuth if neither is found
    ///
    /// # Arguments
    /// * `default_model` - Default model to use (e.g., "gpt-4o")
    pub fn from_env(default_model: Option<String>) -> Result<Self, ProviderError> {
        let client = rig::providers::copilot::Client::from_env()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Ok(Self {
            name: "copilot".to_string(),
            rig_client: client,
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Create a Copilot provider for use in the provider registry.
    ///
    /// Accepts the full set of registry config parameters. Proxy settings are
    /// currently ignored because rig's HTTP client does not expose proxy
    /// configuration.
    pub fn with_proxy(
        api_key: &str,
        api_base: Option<String>,
        default_model: Option<String>,
        _proxy_url: Option<String>,
        _proxy_username: Option<String>,
        _proxy_password: Option<String>,
        _extra_headers: HashMap<String, String>,
    ) -> Result<Self, ProviderError> {
        Self::new(api_key, api_base, default_model)
    }

    /// Validate a GitHub Personal Access Token by attempting to authorize.
    ///
    /// Returns `Ok(())` if the token is valid and Copilot access is available.
    pub async fn validate_pat(token: &str) -> Result<(), ProviderError> {
        let client = rig::providers::copilot::Client::builder()
            .github_access_token(token)
            .build()
            .map_err(|e| ProviderError::AuthError(e.to_string()))?;

        client
            .authorize()
            .await
            .map_err(|e| ProviderError::AuthError(e.to_string()))
    }

    /// Run the OAuth Device Flow, caching tokens in the given directory.
    ///
    /// Rig prints the verification URL and user code to stdout automatically.
    /// Tokens are cached in `token_dir` for subsequent use.
    ///
    /// Returns `Ok(())` on success.
    pub async fn oauth_device_flow(
        token_dir: &std::path::Path,
    ) -> Result<(), ProviderError> {
        let client = rig::providers::copilot::Client::builder()
            .oauth()
            .token_dir(token_dir)
            .build()
            .map_err(|e| ProviderError::AuthError(e.to_string()))?;

        client
            .authorize()
            .await
            .map_err(|e| ProviderError::AuthError(e.to_string()))
    }
}

#[async_trait]
impl LlmProvider for CopilotProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request), fields(provider = "copilot", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        debug!("[copilot] chat request");

        let model = self.rig_client.completion_model(&request.model);
        let rig_request = to_rig_request(request);

        let response = model.completion(rig_request).await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Ok(from_rig_response(response))
    }

    #[instrument(skip(self, request), fields(provider = "copilot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        debug!("[copilot] chat stream request");

        let model = self.rig_client.completion_model(&request.model);
        let rig_request = to_rig_request(request);

        let stream_response = model.stream(rig_request).await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        Ok(from_rig_stream(stream_response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copilot_provider_creation() {
        let provider = CopilotProvider::new("test_token", None, None);
        assert!(provider.is_ok());
        let provider = provider.unwrap();
        assert_eq!(provider.name(), "copilot");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_copilot_provider_custom_model() {
        let provider = CopilotProvider::new("test_token", None, Some("gpt-4-turbo".to_string()));
        assert!(provider.is_ok());
        let provider = provider.unwrap();
        assert_eq!(provider.default_model(), "gpt-4-turbo");
    }

    #[test]
    fn test_copilot_provider_with_api_key() {
        let provider = CopilotProvider::with_api_key("test_api_key", None);
        assert!(provider.is_ok());
        let provider = provider.unwrap();
        assert_eq!(provider.name(), "copilot");
    }
}