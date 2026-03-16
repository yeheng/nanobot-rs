//! GitHub Copilot LLM Provider
//!
//! Implements the `LlmProvider` trait for GitHub Copilot's chat API.
//! Supports both OAuth Device Flow and Personal Access Token authentication.
//!
//! # Example
//!
//! ```ignore
//! // Using PAT
//! let provider = CopilotProvider::new("ghp_xxx", None, "gpt-4o");
//!
//! // Using OAuth-obtained token
//! let provider = CopilotProvider::new("gho_xxx", None, "gpt-4o");
//! ```

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use super::common::build_http_client;
use super::copilot_oauth::CopilotOAuth;
use super::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ToolCall, ToolDefinition,
};

/// Default API base for Copilot
const COPILOT_API_BASE: &str = "https://api.githubcopilot.com";

/// Default model for Copilot
const DEFAULT_MODEL: &str = "gpt-4o";

/// Token refresh buffer (refresh 60 seconds before expiry)
const TOKEN_REFRESH_BUFFER_SECS: u64 = 60;

/// Cached Copilot token with expiry
struct CachedToken {
    token: String,
    expires_at: Instant,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        let now = Instant::now();
        now >= self.expires_at
            || self.expires_at.duration_since(now) < Duration::from_secs(TOKEN_REFRESH_BUFFER_SECS)
    }
}

/// GitHub Copilot provider
///
/// Implements the LlmProvider trait for GitHub Copilot's chat completions API.
/// Handles automatic token refresh for short-lived Copilot JWTs.
pub struct CopilotProvider {
    client: Client,
    github_token: String,
    cached_token: Mutex<Option<CachedToken>>,
    api_base: String,
    default_model: String,
}

impl CopilotProvider {
    /// Create a new Copilot provider
    ///
    /// # Arguments
    /// * `github_token` - GitHub access token (PAT or OAuth token)
    /// * `api_base` - Optional custom API base URL
    /// * `default_model` - Default model to use (e.g., "gpt-4o")
    pub fn new(
        github_token: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
    ) -> Self {
        Self {
            client: build_http_client(true),
            github_token: github_token.into(),
            cached_token: Mutex::new(None),
            api_base: api_base.unwrap_or_else(|| COPILOT_API_BASE.to_string()),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    /// Create a new Copilot provider with proxy configuration
    ///
    /// # Arguments
    /// * `github_token` - GitHub access token (PAT or OAuth token)
    /// * `api_base` - Optional custom API base URL
    /// * `default_model` - Default model to use (e.g., "gpt-4o")
    /// * `proxy_enabled` - Whether to enable HTTP proxy (default: true)
    pub fn with_proxy(
        github_token: impl Into<String>,
        api_base: Option<String>,
        default_model: Option<String>,
        proxy_enabled: bool,
    ) -> Self {
        Self {
            client: build_http_client(proxy_enabled),
            github_token: github_token.into(),
            cached_token: Mutex::new(None),
            api_base: api_base.unwrap_or_else(|| COPILOT_API_BASE.to_string()),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    /// Get a valid Copilot JWT token, refreshing if necessary
    ///
    /// Copilot tokens are short-lived (~30 minutes). This method handles
    /// automatic refresh when the token is expired or about to expire.
    async fn get_copilot_token(&self) -> anyhow::Result<String> {
        // Check if we have a valid cached token
        {
            let cached = self.cached_token.lock().unwrap();
            if let Some(ref token) = *cached {
                if !token.is_expired() {
                    return Ok(token.token.clone());
                }
            }
        }

        // Token expired or missing - refresh it
        self.refresh_copilot_token().await
    }

    /// Exchange GitHub token for a fresh Copilot JWT
    async fn refresh_copilot_token(&self) -> anyhow::Result<String> {
        debug!("Refreshing Copilot token");

        let oauth = CopilotOAuth::with_default_client_id();
        let response = oauth.get_copilot_token(&self.github_token).await?;

        let token = response.token.clone();
        let refresh_in = response.refresh_in;

        // Cache the new token
        {
            let mut cached = self.cached_token.lock().unwrap();
            *cached = Some(CachedToken {
                token: token.clone(),
                expires_at: Instant::now() + Duration::from_secs(refresh_in as u64),
            });
        }

        info!("Copilot token refreshed, refresh in {} seconds", refresh_in);
        Ok(token)
    }

    /// Build headers required for Copilot API requests
    fn build_headers(&self, copilot_token: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", copilot_token).parse().unwrap(),
        );
        headers.insert("copilot-integration-id", "vscode-chat".parse().unwrap());
        headers.insert("Editor-Version", "vscode/1.95.0".parse().unwrap());
        headers.insert(
            "Editor-Plugin-Version",
            "copilot-chat/0.26.7".parse().unwrap(),
        );
        headers.insert("user-agent", "GitHubCopilotChat/0.26.7".parse().unwrap());
        headers.insert("openai-intent", "conversation-panel".parse().unwrap());
        headers.insert("x-github-api-version", "2025-04-01".parse().unwrap());
        headers.insert("x-request-id", Uuid::new_v4().to_string().parse().unwrap());
        headers.insert(
            "x-vscode-user-agent-library-version",
            "electron-fetch".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers
    }
}

#[async_trait]
impl LlmProvider for CopilotProvider {
    fn name(&self) -> &str {
        "copilot"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request), fields(provider = "copilot", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        // Get valid Copilot token (auto-refresh if needed)
        let copilot_token = self.get_copilot_token().await?;

        let url = format!("{}/chat/completions", self.api_base);

        let openai_request = CopilotRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
        };

        tracing::trace!(
            "[copilot] POST {} | request body:\n{}",
            url,
            serde_json::to_string(&openai_request)
                .unwrap_or_else(|e| format!("<failed to serialize request: {}>", e))
        );

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers(&copilot_token))
            .json(&openai_request)
            .send()
            .await?;

        let status = response.status();
        info!("[copilot] response status: {}", status);

        // Handle 401 - token might have expired, try once more
        if status == reqwest::StatusCode::UNAUTHORIZED {
            warn!("[copilot] Token unauthorized, attempting refresh");
            let copilot_token = self.refresh_copilot_token().await?;

            let response = self
                .client
                .post(&url)
                .headers(self.build_headers(&copilot_token))
                .json(&openai_request)
                .send()
                .await?;

            let status = response.status();
            let body = response.text().await?;

            if !status.is_success() {
                anyhow::bail!(
                    "Copilot API error after token refresh: {} - {}",
                    status,
                    body
                );
            }

            return parse_copilot_response(&body);
        }

        let body = response.text().await?;
        info!("[copilot] response body:\n{}", body);

        if !status.is_success() {
            anyhow::bail!("Copilot API error: {} - {}", status, body);
        }

        parse_copilot_response(&body)
    }

    #[instrument(skip(self, request), fields(provider = "copilot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> anyhow::Result<ChatStream> {
        // Get valid Copilot token (auto-refresh if needed)
        let copilot_token = self.get_copilot_token().await?;

        let url = format!("{}/chat/completions", self.api_base);

        let openai_request = CopilotRequest {
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
        };

        tracing::trace!(
            "[copilot] POST {} (stream) | request body:\n{}",
            url,
            serde_json::to_string(&openai_request)
                .unwrap_or_else(|e| format!("<failed to serialize request: {}>", e))
        );

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers(&copilot_token))
            .json(&openai_request)
            .send()
            .await?;

        let status = response.status();
        debug!("[copilot] stream response status: {}", status);

        if !status.is_success() {
            let body = response.text().await?;
            anyhow::bail!("Copilot API error: {} - {}", status, body);
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = super::streaming::parse_sse_stream(byte_stream);

        Ok(Box::pin(chunk_stream))
    }
}

/// Parse Copilot API response
fn parse_copilot_response(body: &str) -> anyhow::Result<ChatResponse> {
    let api_response: CopilotResponse = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("Copilot API response parse error: {} | body: {}", e, body))?;

    let choice = api_response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No choices in Copilot response"))?;

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
        reasoning_content: None, // Copilot doesn't support reasoning_content
        usage: None,
    })
}

/// Parse JSON arguments from string
fn parse_json_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// Copilot API Types (OpenAI-compatible)
// ---------------------------------------------------------------------------

/// Copilot chat request (OpenAI-compatible format)
#[derive(Debug, Clone, Serialize)]
struct CopilotRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

/// Copilot chat response (OpenAI-compatible format)
#[derive(Debug, Clone, Deserialize)]
struct CopilotResponse {
    choices: Vec<CopilotChoice>,
}

/// A choice in the response
#[derive(Debug, Clone, Deserialize)]
struct CopilotChoice {
    message: CopilotMessage,
    #[serde(default)]
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

/// Message in a choice
#[derive(Debug, Clone, Deserialize)]
struct CopilotMessage {
    #[allow(dead_code)]
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<CopilotToolCall>>,
}

/// Tool call in a message
#[derive(Debug, Clone, Deserialize)]
struct CopilotToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    call_type: String,
    function: CopilotFunctionCall,
}

/// Function call
#[derive(Debug, Clone, Deserialize)]
struct CopilotFunctionCall {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copilot_provider_creation() {
        let provider = CopilotProvider::new("test_token", None, None);
        assert_eq!(provider.name(), "copilot");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_copilot_provider_custom_model() {
        let provider = CopilotProvider::new("test_token", None, Some("gpt-4-turbo".to_string()));
        assert_eq!(provider.default_model(), "gpt-4-turbo");
    }

    #[test]
    fn test_parse_copilot_response() {
        let body = r#"{
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "Hello, world!"
                    },
                    "finish_reason": "stop"
                }
            ]
        }"#;

        let response = parse_copilot_response(body).unwrap();
        assert_eq!(response.content, Some("Hello, world!".to_string()));
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_copilot_response_with_tool_calls() {
        let body = r#"{
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_123",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\": \"test.txt\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        }"#;

        let response = parse_copilot_response(body).unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "read_file");
    }
}
