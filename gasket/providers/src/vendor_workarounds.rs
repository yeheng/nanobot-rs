//! Vendor-specific workarounds and provider factories.
//!
//! All non-Copilot provider construction lives here.  MiniMax message
//! normalisation and Moonshot format-switching are the only pieces that
//! cannot be handled by the generic `RigCompletionProvider`.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::instrument;

use crate::rig_provider::RigCompletionProvider;
use crate::{ChatRequest, ChatResponse, ChatStream, LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

const ANTHROPIC_DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Build an Anthropic provider backed by rig.
#[allow(clippy::too_many_arguments)]
pub fn build_anthropic_provider(
    api_key: String,
    api_base: Option<String>,
    default_model: Option<String>,
    default_max_tokens: Option<u32>,
    proxy_url: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    extra_headers: HashMap<String, String>,
) -> RigCompletionProvider<rig::providers::anthropic::Client<crate::logging_http::LoggingHttpClient>> {
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
    let client = builder.build().expect("Failed to create Anthropic client");
    RigCompletionProvider::new("anthropic", default_model.unwrap_or_else(|| ANTHROPIC_DEFAULT_MODEL.to_string()), client)
        .with_max_tokens(default_max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS))
        .with_thinking(true)
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

const GEMINI_DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Build a Gemini provider backed by rig.
pub fn build_gemini_provider(
    api_key: String,
    api_base: Option<String>,
    default_model: Option<String>,
    proxy_url: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    extra_headers: HashMap<String, String>,
) -> RigCompletionProvider<rig::providers::gemini::Client<crate::logging_http::LoggingHttpClient>> {
    let http = crate::common::build_http_client(
        proxy_url.as_deref(),
        proxy_username.as_deref(),
        proxy_password.as_deref(),
    );
    let mut builder = rig::providers::gemini::Client::builder()
        .api_key(api_key)
        .http_client(crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers));
    if let Some(base) = api_base {
        builder = builder.base_url(&base);
    }
    let client = builder.build().expect("Failed to create Gemini client");
    RigCompletionProvider::new("gemini", default_model.unwrap_or_else(|| GEMINI_DEFAULT_MODEL.to_string()), client)
}

// ---------------------------------------------------------------------------
// MiniMax — message normalisation
// ---------------------------------------------------------------------------

const MINIMAX_DEFAULT_MODEL: &str = "MiniMax-M2.7";

/// Convert system messages to user messages.
pub fn convert_system_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
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
pub fn merge_consecutive_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    let mut merged: Vec<crate::ChatMessage> = Vec::new();
    for msg in messages {
        if let Some(last) = merged.last_mut() {
            if last.role == msg.role && !matches!(msg.role, crate::MessageRole::Tool) {
                if matches!(msg.role, crate::MessageRole::Assistant)
                    && (msg.tool_calls.is_some() || last.tool_calls.is_some())
                {
                    merged.push(msg);
                    continue;
                }
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

/// Sanitize messages for MiniMax input.
pub fn sanitize_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    messages
        .into_iter()
        .map(|mut msg| {
            msg.reasoning_content = None;
            if matches!(msg.role, crate::MessageRole::Tool) {
                msg.name = None;
            }
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

/// Full MiniMax message normalization pipeline.
pub fn normalize_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    let messages = convert_system_messages(messages);
    let messages = merge_consecutive_messages(messages);
    sanitize_messages(messages)
}

/// Build a MiniMax provider backed by rig.
pub fn build_minimax_provider(
    api_key: String,
    api_base: Option<String>,
    default_model: Option<String>,
    proxy_url: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    extra_headers: HashMap<String, String>,
) -> RigCompletionProvider<rig::providers::minimax::Client<crate::logging_http::LoggingHttpClient>> {
    let http = crate::common::build_http_client(
        proxy_url.as_deref(),
        proxy_username.as_deref(),
        proxy_password.as_deref(),
    );
    let mut builder = rig::providers::minimax::Client::builder()
        .api_key(api_key)
        .http_client(crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers));
    if let Some(base) = api_base {
        builder = builder.base_url(&base);
    }
    let client = builder.build().expect("Failed to create MiniMax client");
    RigCompletionProvider::new("minimax", default_model.unwrap_or_else(|| MINIMAX_DEFAULT_MODEL.to_string()), client)
        .with_normalizer(normalize_messages)
}

// ---------------------------------------------------------------------------
// Moonshot — runtime format switching
// ---------------------------------------------------------------------------

const MOONSHOT_API_BASE: &str = "https://api.moonshot.cn/v1";
const MOONSHOT_DEFAULT_MODEL: &str = "kimi-k2.6";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFormat {
    OpenAI,
    Anthropic,
}

/// Moonshot provider with runtime OpenAI / Anthropic format switching.
pub struct MoonshotProvider {
    openai_provider: RigCompletionProvider<
        rig::providers::moonshot::Client<crate::logging_http::LoggingHttpClient>,
    >,
    anthropic_provider: Option<
        RigCompletionProvider<
            rig::providers::moonshot::AnthropicClient<crate::logging_http::LoggingHttpClient>,
        >,
    >,
    api_base: String,
}

impl MoonshotProvider {
    fn api_format(&self) -> ApiFormat {
        if self.api_base.contains("/coding") || self.api_base.contains("/anthropic") {
            ApiFormat::Anthropic
        } else {
            ApiFormat::OpenAI
        }
    }

    /// Build a Moonshot provider with full configuration.
    pub fn with_config(
        api_key: String,
        api_base: Option<String>,
        default_model: Option<String>,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        let final_api_base = api_base.unwrap_or_else(|| MOONSHOT_API_BASE.to_string());
        let model = default_model.unwrap_or_else(|| MOONSHOT_DEFAULT_MODEL.to_string());

        let http = crate::common::build_http_client(
            proxy_url.as_deref(),
            proxy_username.as_deref(),
            proxy_password.as_deref(),
        );

        let logging = crate::logging_http::LoggingHttpClient::new(http.clone())
            .with_extra_headers(extra_headers.clone());

        let openai_client = rig::providers::moonshot::Client::builder()
            .api_key(api_key.clone())
            .base_url(&final_api_base)
            .http_client(logging)
            .build()
            .expect("Failed to create Moonshot client");

        let anthropic_client = if final_api_base.contains("/coding")
            || final_api_base.contains("/anthropic")
        {
            Some(
                rig::providers::moonshot::AnthropicClient::builder()
                    .api_key(api_key)
                    .base_url(&final_api_base)
                    .http_client(
                        crate::logging_http::LoggingHttpClient::new(http).with_extra_headers(extra_headers),
                    )
                    .build()
                    .expect("Failed to create Moonshot Anthropic client"),
            )
        } else {
            None
        };

        Self {
            openai_provider: RigCompletionProvider::new("moonshot", model.clone(), openai_client),
            anthropic_provider: anthropic_client.map(|c| {
                RigCompletionProvider::new("moonshot", model, c)
            }),
            api_base: final_api_base,
        }
    }
}

#[async_trait]
impl LlmProvider for MoonshotProvider {
    fn name(&self) -> &str {
        "moonshot"
    }

    fn default_model(&self) -> &str {
        self.openai_provider.default_model()
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let format = self.api_format();
        let provider: &dyn LlmProvider = match format {
            ApiFormat::OpenAI => &self.openai_provider,
            ApiFormat::Anthropic => self
                .anthropic_provider
                .as_ref()
                .ok_or_else(|| ProviderError::Other("Anthropic client not available".to_string()))?,
        };
        provider.chat(request).await
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let format = self.api_format();
        let provider: &dyn LlmProvider = match format {
            ApiFormat::OpenAI => &self.openai_provider,
            ApiFormat::Anthropic => self
                .anthropic_provider
                .as_ref()
                .ok_or_else(|| ProviderError::Other("Anthropic client not available".to_string()))?,
        };
        provider.chat_stream(request).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ChatRequest, ToolCall};
    use serde_json::{Value, json};

    // --- MiniMax ---

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

        let normalized = normalize_messages(request.messages);
        let msgs: Vec<Value> = normalized
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(
            msgs[0]["content"],
            "System prompt 1\n\nSystem prompt 2\n\nHello\n\nWorld"
        );
    }

    #[test]
    fn test_system_messages_converted_to_user() {
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

        let normalized = normalize_messages(request.messages);
        let msgs: Vec<Value> = normalized
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "You are helpful\n\nHello");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    #[test]
    fn test_tool_messages_not_merged() {
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

        let normalized = normalize_messages(request.messages);
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[0].role, crate::MessageRole::Assistant);
        assert_eq!(normalized[1].role, crate::MessageRole::Tool);
        assert_eq!(normalized[2].role, crate::MessageRole::Tool);
    }

    // --- Moonshot ---

    #[test]
    fn test_api_format_detection() {
        let openai = MoonshotProvider::with_config(
            "test-key".to_string(),
            Some("https://api.moonshot.cn/v1".to_string()),
            None,
            None,
            None,
            None,
            HashMap::new(),
        );
        assert_eq!(openai.api_format(), ApiFormat::OpenAI);

        let coding = MoonshotProvider::with_config(
            "test-key".to_string(),
            Some("https://api.moonshot.cn/coding".to_string()),
            None,
            None,
            None,
            None,
            HashMap::new(),
        );
        assert_eq!(coding.api_format(), ApiFormat::Anthropic);

        let anthropic = MoonshotProvider::with_config(
            "test-key".to_string(),
            Some("https://api.moonshot.cn/anthropic".to_string()),
            None,
            None,
            None,
            None,
            HashMap::new(),
        );
        assert_eq!(anthropic.api_format(), ApiFormat::Anthropic);
    }
}
