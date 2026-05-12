//! Moonshot (Kimi) LLM provider
//!
//! Uses rig's Moonshot client for API communication.
//!
//! Supports both OpenAI-compatible and Anthropic-format endpoints.
//!
//! # API Endpoint
//!
//! `POST {api_base}/chat/completions`  (OpenAI format, api_base ends with `/v1`)
//! `POST {api_base}/messages`          (Anthropic format, api_base ends with `/coding` or `/anthropic`)

use crate::base::{ChatStreamChunk, ChatStreamDelta, FinishReason, ToolCallDelta};
use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{ChatRequest, ChatResponse, ChatStream, LlmProvider, ProviderError};
use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, instrument};

/// Default API base for Moonshot
const MOONSHOT_API_BASE: &str = "https://api.moonshot.cn/v1";

/// Default model for Moonshot
const DEFAULT_MODEL: &str = "kimi-k2.6";

/// API format detected from the api_base URL suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFormat {
    /// OpenAI-compatible format (`/v1`)
    OpenAI,
    /// Anthropic Messages API format (`/coding`, `/anthropic`)
    Anthropic,
}

/// Moonshot provider using rig's client
pub struct MoonshotProvider {
    /// Rig Moonshot client (OpenAI-compatible)
    rig_client: rig::providers::moonshot::Client,

    /// Rig Moonshot Anthropic client (for Anthropic-format endpoints)
    rig_anthropic_client: Option<rig::providers::moonshot::AnthropicClient>,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,

    /// Optional default cache_tag
    default_cache_tag: Option<String>,

    /// Optional default user_id
    default_user_id: Option<String>,

    /// Extra HTTP headers to send with every request
    extra_headers: HashMap<String, String>,
}

impl MoonshotProvider {
    /// Resolve the effective API base URL.
    ///
    /// For `/coding` and `/anthropic` endpoints, if the URL does not already
    /// end with `/v1`, we append `/v1` automatically.
    fn resolved_api_base(&self) -> String {
        let base = &self.api_base;
        if (base.contains("/coding") || base.contains("/anthropic")) && !base.ends_with("/v1") {
            format!("{}/v1", base)
        } else {
            base.clone()
        }
    }

    /// Detect API format from the resolved API base URL.
    ///
    /// Priority:
    /// 1. If URL contains `/coding` or `/anthropic` → Anthropic format
    /// 2. If URL ends with `/v1` → OpenAI format
    /// 3. Otherwise default to OpenAI format
    fn api_format(&self) -> ApiFormat {
        if self.api_base.contains("/coding") || self.api_base.contains("/anthropic") {
            ApiFormat::Anthropic
        } else {
            ApiFormat::OpenAI
        }
    }

    /// Create a new Moonshot provider
    pub fn new(api_key: String) -> Self {
        let rig_client = rig::providers::moonshot::Client::builder()
            .api_key(api_key.clone())
            .build()
            .expect("Failed to create Moonshot client");
        Self {
            rig_client,
            rig_anthropic_client: None,
            api_base: MOONSHOT_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            default_cache_tag: None,
            default_user_id: None,
            extra_headers: HashMap::new(),
        }
    }

    /// Create with proxy configuration
    pub fn with_proxy(
        api_key: String,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        let mut builder = rig::providers::moonshot::Client::builder().api_key(api_key.clone());
        if let Some(url) = proxy_url {
            builder = builder.base_url(&url);
        }
        Self {
            rig_client: builder.build().expect("Failed to create Moonshot client"),
            rig_anthropic_client: None,
            api_base: MOONSHOT_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            default_cache_tag: None,
            default_user_id: None,
            extra_headers: HashMap::new(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        let rig_client = rig::providers::moonshot::Client::builder()
            .api_key(api_key.clone())
            .base_url(&api_base)
            .build()
            .expect("Failed to create Moonshot client");

        // If using Anthropic format, also create the Anthropic client
        let rig_anthropic_client = if api_base.contains("/coding") || api_base.contains("/anthropic") {
            Some(
                rig::providers::moonshot::AnthropicClient::builder()
                    .api_key(api_key.clone())
                    .base_url(&api_base)
                    .build()
                    .expect("Failed to create Moonshot Anthropic client"),
            )
        } else {
            None
        };

        Self {
            rig_client,
            rig_anthropic_client,
            api_base,
            default_model: DEFAULT_MODEL.to_string(),
            default_cache_tag: None,
            default_user_id: None,
            extra_headers: HashMap::new(),
        }
    }

    /// Create with full configuration
    #[allow(clippy::too_many_arguments)]
    pub fn with_config(
        api_key: String,
        api_base: Option<String>,
        default_model: Option<String>,
        default_cache_tag: Option<String>,
        default_user_id: Option<String>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        let final_api_base = api_base
            .unwrap_or_else(|| MOONSHOT_API_BASE.to_string());

        let http = crate::common::build_http_client(
            proxy_url.as_deref(),
            proxy_username.as_deref(),
            proxy_password.as_deref(),
        );

        let mut builder = rig::providers::moonshot::Client::builder()
            .api_key(api_key.clone())
            .base_url(&final_api_base)
            .http_client(http.clone());

        let rig_anthropic_client = if final_api_base.contains("/coding") || final_api_base.contains("/anthropic") {
            Some(
                rig::providers::moonshot::AnthropicClient::builder()
                    .api_key(api_key.clone())
                    .base_url(&final_api_base)
                    .http_client(http)
                    .build()
                    .expect("Failed to create Moonshot Anthropic client"),
            )
        } else {
            None
        };

        Self {
            rig_client: builder.build().expect("Failed to create Moonshot client"),
            rig_anthropic_client,
            api_base: final_api_base,
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_cache_tag,
            default_user_id,
            extra_headers,
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Set default cache_tag
    pub fn with_cache_tag(mut self, cache_tag: Option<String>) -> Self {
        self.default_cache_tag = cache_tag;
        self
    }

    /// Set default user_id
    pub fn with_user_id(mut self, user_id: Option<String>) -> Self {
        self.default_user_id = user_id;
        self
    }

    /// Parse Moonshot streaming SSE chunk (OpenAI format)
    fn parse_openai_stream_chunk(&self, value: Value) -> ChatStreamChunk {
        let choices = value["choices"].as_array().cloned().unwrap_or_default();

        let choice = choices.into_iter().next();

        let Some(choice) = choice else {
            return ChatStreamChunk {
                delta: ChatStreamDelta::default(),
                finish_reason: None,
                usage: None,
            };
        };

        let delta = &choice["delta"];

        // Extract reasoning content
        let reasoning_content = delta["reasoning_content"].as_str().map(String::from);

        // Extract content
        let content = delta["content"].as_str().map(String::from);

        // Extract tool calls
        let tool_calls: Vec<ToolCallDelta> = delta["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|tc| {
                Some(ToolCallDelta {
                    index: tc["index"].as_u64()? as usize,
                    id: tc["id"].as_str().map(String::from),
                    function_name: tc["function"]["name"].as_str().map(String::from),
                    function_arguments: tc["function"]["arguments"].as_str().map(String::from),
                })
            })
            .collect();

        let finish_reason = choice["finish_reason"].as_str().map(|r| match r {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_calls" => FinishReason::ToolCalls,
            other => FinishReason::Other(other.to_string()),
        });

        let usage = value["usage"].as_object().map(|u| crate::Usage {
            input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        });

        ChatStreamChunk {
            delta: ChatStreamDelta {
                content,
                reasoning_content,
                tool_calls,
            },
            finish_reason,
            usage,
        }
    }
}

#[async_trait]
impl LlmProvider for MoonshotProvider {
    fn name(&self) -> &str {
        "moonshot"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let format = self.api_format();
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let rig_request = to_rig_request(request);

        let response = match format {
            ApiFormat::OpenAI => {
                let rig_response = self
                    .rig_client
                    .completion_model(&model)
                    .completion(rig_request)
                    .await
                    .map_err(|e| {
                        debug!("[moonshot] rig error: {}", e);
                        ProviderError::Other(e.to_string())
                    })?;
                from_rig_response(rig_response)
            }
            ApiFormat::Anthropic => {
                let client = self.rig_anthropic_client.as_ref()
                    .ok_or_else(|| ProviderError::Other("Anthropic client not available".to_string()))?;
                let rig_response = client
                    .completion_model(&model)
                    .completion(rig_request)
                    .await
                    .map_err(|e| {
                        debug!("[moonshot] rig anthropic error: {}", e);
                        ProviderError::Other(e.to_string())
                    })?;
                from_rig_response(rig_response)
            }
        };

        Ok(response)
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let format = self.api_format();
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let rig_request = to_rig_request(request);

        let stream: ChatStream = match format {
            ApiFormat::OpenAI => {
                let streamed = self.rig_client
                    .completion_model(&model)
                    .stream(rig_request)
                    .await
                    .map_err(|e| {
                        debug!("[moonshot] rig stream error: {}", e);
                        ProviderError::Other(e.to_string())
                    })?;
                from_rig_stream(streamed)
            }
            ApiFormat::Anthropic => {
                let client = self.rig_anthropic_client.as_ref()
                    .ok_or_else(|| ProviderError::Other("Anthropic client not available".to_string()))?;
                let streamed = client
                    .completion_model(&model)
                    .stream(rig_request)
                    .await
                    .map_err(|e| {
                        debug!("[moonshot] rig anthropic stream error: {}", e);
                        ProviderError::Other(e.to_string())
                    })?;
                from_rig_stream(streamed)
            }
        };

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = MoonshotProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "moonshot");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
        assert_eq!(provider.api_format(), ApiFormat::OpenAI);
    }

    #[test]
    fn test_api_format_detection() {
        let openai = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/v1".to_string(),
        );
        assert_eq!(openai.api_format(), ApiFormat::OpenAI);

        let coding = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/coding".to_string(),
        );
        assert_eq!(coding.api_format(), ApiFormat::Anthropic);

        let anthropic = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/anthropic".to_string(),
        );
        assert_eq!(anthropic.api_format(), ApiFormat::Anthropic);
    }

    #[test]
    fn test_custom_model() {
        let provider = MoonshotProvider::new("test-key".to_string())
            .with_model("kimi-k2".to_string());
        assert_eq!(provider.default_model(), "kimi-k2");
    }
}