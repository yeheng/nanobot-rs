//! MiniMax LLM provider
//!
//! Uses rig's MiniMax client for API communication with provider-specific
//! message normalization.
//!
//! # API Notes
//!
//! - Endpoint: `POST /v1/chat/completions` (OpenAI-compatible)
//! - Auth: `Authorization: Bearer` header
//! - Thinking content is returned in `reasoning_details` field when `reasoning_split=true`
//! - Streaming uses standard SSE with MiniMax-specific fields

use crate::base::ChatStream;
use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{ChatRequest, ChatResponse, LlmProvider, ProviderError};
use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use std::collections::HashMap;
use tracing::{debug, instrument};

/// Default API base for MiniMax
const MINIMAX_API_BASE: &str = "https://api.minimaxi.com/v1";

/// Default model for MiniMax
const DEFAULT_MODEL: &str = "MiniMax-M2.7";

/// Convert system messages to user messages.
///
/// MiniMax API does not support the `system` role (error 2013:
/// "invalid message role: system"). We collect all system content and prepend it
/// to the first user message, or create a new user message if none exists.
fn convert_system_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut result: Vec<crate::ChatMessage> = Vec::new();

    for msg in messages {
        if matches!(msg.role, crate::MessageRole::System) {
            if let Some(content) = &msg.content {
                system_parts.push(content.clone());
            }
        } else {
            result.push(msg);
        }
    }

    if !system_parts.is_empty() {
        let system_text = system_parts.join("\n\n");
        if let Some(first_user) = result
            .iter_mut()
            .find(|m| matches!(m.role, crate::MessageRole::User) && m.tool_call_id.is_none())
        {
            let new_content = if let Some(content) = &first_user.content {
                format!("{}\n\n{}", system_text, content)
            } else {
                system_text
            };
            first_user.content = Some(new_content);
        } else {
            result.insert(0, crate::ChatMessage::user(system_text));
        }
    }

    result
}

/// Merge consecutive messages with the same role.
///
/// MiniMax API rejects multiple consecutive messages with the same role (error 2013).
/// This merges their content with a double newline separator.
///
/// **Important:**
/// - Tool messages are never merged. Each tool result must retain its
///   own `tool_call_id` to match the corresponding assistant `tool_call`.
/// - Assistant messages with `tool_calls` are never merged. Merging would
///   silently drop the `tool_calls` field, breaking the tool-call / tool-result
///   pairing and causing MiniMax error 2013 ("tool id not found").
fn merge_consecutive_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    let mut merged: Vec<crate::ChatMessage> = Vec::new();
    for msg in messages {
        if let Some(last) = merged.last_mut() {
            // Never merge tool messages — each has a unique tool_call_id that
            // must match a specific assistant tool_call.
            if last.role == msg.role && !matches!(msg.role, crate::MessageRole::Tool) {
                // Never merge an assistant message that carries tool_calls.
                // The merge only combines `content`; `tool_calls` would be lost,
                // breaking the pairing with subsequent tool results.
                if matches!(msg.role, crate::MessageRole::Assistant)
                    && (msg.tool_calls.is_some() || last.tool_calls.is_some())
                {
                    merged.push(msg);
                    continue;
                }

                // Merge content
                match (&mut last.content, &msg.content) {
                    (Some(ref mut a), Some(b)) => {
                        a.push('\n');
                        a.push('\n');
                        a.push_str(b);
                    }
                    (None, Some(b)) => {
                        last.content = Some(b.clone());
                    }
                    _ => {}
                }
                continue;
            }
        }
        merged.push(msg);
    }
    merged
}

/// Sanitize messages for MiniMax input — strip fields that cause error 2013.
///
/// 1. `reasoning_content` — MiniMax returns this in responses but rejects it
///    in input, which breaks tool_call parsing ("tool id not found").
/// 2. `name` on tool messages — OpenAI's `role: "tool"` format does not
///    include `name`; MiniMax may reject it as an unknown field.
/// 3. Missing `content` on assistant tool-call messages — MiniMax requires
///    `content` to be present (even if empty) when `tool_calls` is set.
fn sanitize_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    messages
        .into_iter()
        .map(|mut msg| {
            msg.reasoning_content = None;

            // Strip `name` from tool messages — not valid in OpenAI tool format.
            if matches!(msg.role, crate::MessageRole::Tool) {
                msg.name = None;
            }

            // Ensure assistant messages with tool_calls have a content field.
            if matches!(msg.role, crate::MessageRole::Assistant)
                && msg.tool_calls.is_some()
                && msg.content.is_none()
            {
                msg.content = Some(String::new());
            }

            msg
        })
        .collect()
}

/// MiniMax provider using rig's client
pub struct MinimaxProvider {
    /// Rig MiniMax client
    rig_client: rig::providers::minimax::Client<crate::logging_http::LoggingHttpClient>,

    /// API key
    api_key: String,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,
}

impl MinimaxProvider {
    /// Create a new Minimax provider
    pub fn new(api_key: String) -> Self {
        let rig_client = rig::providers::minimax::Client::builder()
            .api_key(api_key.clone())
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Minimax client");
        Self {
            rig_client,
            api_key,
            api_base: MINIMAX_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Create with proxy configuration
    pub fn with_proxy(
        api_key: String,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        let mut builder = rig::providers::minimax::Client::builder().api_key(api_key.clone());
        if let Some(url) = proxy_url {
            builder = builder.base_url(&url);
        }
        Self {
            rig_client: builder
                .http_client(crate::logging_http::LoggingHttpClient::default())
                .build()
                .expect("Failed to create Minimax client"),
            api_key,
            api_base: MINIMAX_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        let rig_client = rig::providers::minimax::Client::builder()
            .api_key(api_key.clone())
            .base_url(&api_base)
            .http_client(crate::logging_http::LoggingHttpClient::default())
            .build()
            .expect("Failed to create Minimax client");
        Self {
            rig_client,
            api_key,
            api_base,
            default_model: DEFAULT_MODEL.to_string(),
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
        let mut builder = rig::providers::minimax::Client::builder()
            .api_key(api_key.clone())
            .http_client(crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers));
        if let Some(ref base) = api_base {
            builder = builder.base_url(base);
        }
        Self {
            rig_client: builder.build().expect("Failed to create Minimax client"),
            api_key,
            api_base: api_base.unwrap_or_else(|| MINIMAX_API_BASE.to_string()),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Normalize messages for MiniMax API requirements
    fn normalize_messages(&self, messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
        let messages = convert_system_messages(messages);
        let messages = merge_consecutive_messages(messages);
        sanitize_messages(messages)
    }

}

#[async_trait]
impl LlmProvider for MinimaxProvider {
    fn name(&self) -> &str {
        "minimax"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn normalize_messages(&self, messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
        self.normalize_messages(messages)
    }

    #[instrument(skip(self, request), fields(provider = "minimax", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        // Normalize messages for MiniMax requirements
        let normalized_request = ChatRequest {
            messages: self.normalize_messages(request.messages),
            ..request
        };

        let rig_request = to_rig_request(normalized_request);
        let rig_response = self
            .rig_client
            .completion_model(&model)
            .completion(rig_request)
            .await
            .map_err(|e| {
                debug!("[minimax] rig error: {}", e);
                ProviderError::Other(e.to_string())
            })?;

        Ok(from_rig_response(rig_response))
    }

    #[instrument(skip(self, request), fields(provider = "minimax", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        // Normalize messages for MiniMax requirements
        let normalized_request = ChatRequest {
            messages: self.normalize_messages(request.messages),
            ..request
        };

        let rig_request = to_rig_request(normalized_request);
        let stream = self
            .rig_client
            .completion_model(&model)
            .stream(rig_request)
            .await
            .map_err(|e| {
                debug!("[minimax] rig stream error: {}", e);
                ProviderError::Other(e.to_string())
            })?;

        Ok(from_rig_stream(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ToolCall};
    use serde_json::{Value, json};

    #[test]
    fn test_provider_creation() {
        let provider = MinimaxProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_custom_model() {
        let provider = MinimaxProvider::new("test-key".to_string())
            .with_model("MiniMax-M2.5".to_string());
        assert_eq!(provider.default_model(), "MiniMax-M2.5");
    }

    #[test]
    fn test_merge_consecutive_system_messages() {
        let request = ChatRequest {
            model: "MiniMax-M2.7".to_string(),
            messages: vec![
                ChatMessage::system("System prompt 1"),
                ChatMessage::system("System prompt 2"),
                ChatMessage::user("Hello"),
                ChatMessage::user("World"),
            ],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let normalized = MinimaxProvider::new("test-key".to_string())
            .normalize_messages(request.messages);
        let msgs: Vec<Value> = normalized
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        // System messages are converted to user and then merged with consecutive user messages.
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(
            msgs[0]["content"],
            "System prompt 1\n\nSystem prompt 2\n\nHello\n\nWorld"
        );
    }

    #[test]
    fn test_system_messages_converted_to_user() {
        let provider = MinimaxProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "MiniMax-M2.7".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
                ChatMessage::assistant("Hi!"),
            ],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let normalized = provider.normalize_messages(request.messages);
        let msgs: Vec<Value> = normalized
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        // System message must be converted to user — MiniMax rejects `role: system`.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "You are helpful\n\nHello");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    #[test]
    fn test_tool_messages_not_merged() {
        let provider = MinimaxProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "MiniMax-M2.7".to_string(),
            messages: vec![
                ChatMessage::assistant_with_tools(
                    None,
                    vec![
                        ToolCall::new("call_1", "web_fetch", json!({"url": "a"})),
                        ToolCall::new("call_2", "web_fetch", json!({"url": "b"})),
                    ],
                    None,
                ),
                ChatMessage::tool_result("call_1", "web_fetch", "Result A"),
                ChatMessage::tool_result("call_2", "web_fetch", "Result B"),
            ],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let normalized = provider.normalize_messages(request.messages);

        // Tool messages must remain separate so each retains its tool_call_id.
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[0].role, crate::MessageRole::Assistant);
        assert_eq!(normalized[1].role, crate::MessageRole::Tool);
        assert_eq!(normalized[2].role, crate::MessageRole::Tool);
    }
}