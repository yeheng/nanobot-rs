//! Anthropic Claude LLM provider
//!
//! Implements the native Anthropic Messages API (not OpenAI-compatible).
//!
//! # API Differences from OpenAI
//!
//! - Endpoint: `POST /v1/messages` (not `/v1/chat/completions`)
//! - Auth: `x-api-key` header (not `Authorization: Bearer`)
//! - System message is a top-level `system` field (not in `messages` array)
//! - Messages only support `user` and `assistant` roles
//! - Tool calls are `tool_use` content blocks; tool results are `tool_result` content blocks
//! - Response uses `content` array of blocks instead of `choices`
//! - Streaming uses event types: `message_start`, `content_block_start`, `content_block_delta`, etc.

use crate::base::{ChatStream, ChatStreamChunk, ChatStreamDelta, FinishReason, ToolCallDelta};
use crate::common::build_http_client;
use crate::streaming::sse_lines;
use crate::{ChatRequest, ChatResponse, LlmProvider, ToolCall};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, error, instrument, warn};

/// Default API base for Anthropic
const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";

/// Anthropic API version header
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default model for Anthropic
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Default max tokens for Anthropic (required parameter)
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic provider using the native Messages API
pub struct AnthropicProvider {
    /// HTTP client
    client: Client,

    /// API key
    api_key: String,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,

    /// Default max tokens
    default_max_tokens: u32,

    /// Extra HTTP headers to send with every request
    extra_headers: HashMap<String, String>,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_http_client(None, None, None),
            api_key,
            api_base: ANTHROPIC_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
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
        Self {
            client: build_http_client(
                proxy_url.as_deref(),
                proxy_username.as_deref(),
                proxy_password.as_deref(),
            ),
            api_key,
            api_base: ANTHROPIC_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
            extra_headers: HashMap::new(),
        }
    }

    /// Create with custom API base URL
    pub fn with_api_base(api_key: String, api_base: String) -> Self {
        Self {
            client: build_http_client(None, None, None),
            api_key,
            api_base,
            default_model: DEFAULT_MODEL.to_string(),
            default_max_tokens: DEFAULT_MAX_TOKENS,
            extra_headers: HashMap::new(),
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
        Self {
            client: build_http_client(
                proxy_url.as_deref(),
                proxy_username.as_deref(),
                proxy_password.as_deref(),
            ),
            api_key,
            api_base: api_base.unwrap_or_else(|| ANTHROPIC_API_BASE.to_string()),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_max_tokens: default_max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            extra_headers,
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

    /// Build Anthropic Messages API request body
    fn build_request(&self, request: ChatRequest) -> Value {
        let mut messages = Vec::new();
        let mut system_instruction = None;

        for msg in request.messages {
            match msg.role {
                crate::MessageRole::System => {
                    system_instruction = msg.content;
                }
                crate::MessageRole::User => {
                    // Check if this is a tool result message
                    if let Some(ref tool_call_id) = msg.tool_call_id {
                        let content = json!({
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": msg.content.unwrap_or_default(),
                        });
                        messages.push(json!({
                            "role": "user",
                            "content": vec![content],
                        }));
                    } else {
                        messages.push(json!({
                            "role": "user",
                            "content": msg.content.unwrap_or_default(),
                        }));
                    }
                }
                crate::MessageRole::Assistant => {
                    let mut content_blocks = Vec::new();

                    // Add text content if present
                    if let Some(text) = &msg.content {
                        if !text.is_empty() {
                            content_blocks.push(json!({
                                "type": "text",
                                "text": text,
                            }));
                        }
                    }

                    // Add tool_use blocks for assistant tool calls
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.function.name,
                                "input": tc.function.arguments,
                            }));
                        }
                    }

                    if !content_blocks.is_empty() {
                        messages.push(json!({
                            "role": "assistant",
                            "content": content_blocks,
                        }));
                    }
                }
                crate::MessageRole::Tool => {
                    // Tool messages in OpenAI format → tool_result blocks in Anthropic
                    let tool_use_id = msg.tool_call_id.unwrap_or_default();
                    let content = json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": msg.content.unwrap_or_default(),
                    });
                    messages.push(json!({
                        "role": "user",
                        "content": vec![content],
                    }));
                }
            }
        }

        let mut body = json!({
            "model": if request.model.is_empty() {
                &self.default_model
            } else {
                &request.model
            },
            "max_tokens": request.max_tokens.unwrap_or(self.default_max_tokens),
            "messages": messages,
        });

        if let Some(system) = system_instruction {
            body["system"] = json!(system);
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        // Convert tools to Anthropic format
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                let anthropic_tools: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.function.name,
                            "description": t.function.description,
                            "input_schema": t.function.parameters,
                        })
                    })
                    .collect();
                body["tools"] = json!(anthropic_tools);
            }
        }

        debug!(
            "Anthropic request: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse Anthropic response into ChatResponse
    fn parse_response(&self, response: Value) -> Result<ChatResponse, crate::ProviderError> {
        debug!(
            "Anthropic response: {}",
            serde_json::to_string(&response)
                .unwrap_or_else(|_| "<serialization error>".to_string())
        );

        // Check for errors
        if let Some(error) = response.get("error") {
            return Err(crate::ProviderError::ApiError {
                status_code: error["type"]
                    .as_str()
                    .map(|t| if t == "rate_limit_error" { 429 } else { 400 })
                    .unwrap_or(400),
                message: format!("Anthropic API error: {}", error),
            });
        }

        let content_blocks = response["content"].as_array().cloned().unwrap_or_default();

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in &content_blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        text_parts.push(text.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block["id"].as_str().unwrap_or("").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    tool_calls.push(ToolCall::new(id, name, input));
                }
                _ => {}
            }
        }

        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        let usage = response.get("usage").map(|u| crate::Usage {
            input_tokens: u["input_tokens"].as_u64().unwrap_or(0) as usize,
            output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as usize,
            total_tokens: u["input_tokens"].as_u64().unwrap_or(0) as usize
                + u["output_tokens"].as_u64().unwrap_or(0) as usize,
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            reasoning_content: None,
            usage,
        })
    }

    /// Build headers for Anthropic API requests
    fn build_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", self.api_key.parse().unwrap());
        headers.insert("anthropic-version", ANTHROPIC_VERSION.parse().unwrap());
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        // Apply user-configured extra headers
        for (key, value) in &self.extra_headers {
            if let Ok(header_name) = key.parse::<reqwest::header::HeaderName>() {
                if let Ok(header_value) = value.parse::<reqwest::header::HeaderValue>() {
                    headers.insert(header_name, header_value);
                }
            }
        }

        headers
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

    #[instrument(skip(self, request), fields(provider = "anthropic", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let url = format!("{}/messages", self.api_base);
        let body = self.build_request(request);

        debug!("[anthropic] POST {}", url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::ProviderError::NetworkError(format!("Anthropic request failed: {}", e))
            })?;

        let status = response.status();
        debug!("[anthropic] response status: {}", status);

        let response_text = response.text().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Failed to read Anthropic response: {}", e))
        })?;

        if !status.is_success() {
            error!("[anthropic] response body:\n{}", response_text);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, response_text),
            });
        }

        let response_value: Value = serde_json::from_str(&response_text).map_err(|e| {
            crate::ProviderError::ParseError(format!(
                "Anthropic API response parse error: {} | body: {}",
                e, response_text
            ))
        })?;

        self.parse_response(response_value)
    }

    #[instrument(skip(self, request), fields(provider = "anthropic", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let url = format!("{}/messages", self.api_base);
        let mut body = self.build_request(request);
        body["stream"] = json!(true);

        debug!("[anthropic] POST {} (stream)", url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::ProviderError::NetworkError(format!(
                    "Anthropic stream request failed: {}",
                    e
                ))
            })?;

        let status = response.status();
        debug!("[anthropic] stream response status: {}", status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!(
                    "Failed to read Anthropic stream response: {}",
                    e
                ))
            })?;
            error!("[anthropic] response body:\n{}", body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, body),
            });
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = parse_anthropic_sse_stream(byte_stream);
        Ok(Box::pin(chunk_stream))
    }
}

/// Parse an Anthropic SSE byte stream into `ChatStreamChunk`s.
///
/// Anthropic SSE events have the format:
/// ```text
/// event: content_block_delta
/// data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}
///
/// ```
fn parse_anthropic_sse_stream(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<ChatStreamChunk, crate::ProviderError>> + Send + 'static
{
    let lines = sse_lines(byte_stream);

    futures_util::stream::unfold(
        (Box::pin(lines), None::<String>),
        |(mut lines, mut event_type)| async move {
            loop {
                let line = match lines.next().await {
                    Some(Ok(line)) => line,
                    Some(Err(e)) => {
                        return Some((
                            Err(crate::ProviderError::NetworkError(format!(
                                "Anthropic SSE stream error: {}",
                                e
                            ))),
                            (lines, event_type),
                        ));
                    }
                    None => return None,
                };

                let line = line.trim();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(ev) = line.strip_prefix("event: ") {
                    event_type = Some(ev.to_string());
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return None;
                    }

                    let ev = event_type.take();
                    match serde_json::from_str::<Value>(data) {
                        Ok(value) => {
                            let chunk = convert_anthropic_chunk(value, ev.as_deref());
                            if let Some(chunk) = chunk {
                                return Some((Ok(chunk), (lines, event_type)));
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse Anthropic SSE chunk: {} | data: {}",
                                e, data
                            );
                        }
                    }
                }
            }
        },
    )
}

/// Convert an Anthropic SSE event into a `ChatStreamChunk`.
fn convert_anthropic_chunk(value: Value, event_type: Option<&str>) -> Option<ChatStreamChunk> {
    match event_type {
        Some("message_start") => {
            // message_start contains usage info
            let usage = value["message"].get("usage").map(|u| crate::Usage {
                input_tokens: u["input_tokens"].as_u64().unwrap_or(0) as usize,
                output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as usize,
                total_tokens: u["input_tokens"].as_u64().unwrap_or(0) as usize
                    + u["output_tokens"].as_u64().unwrap_or(0) as usize,
            });
            Some(ChatStreamChunk {
                delta: ChatStreamDelta::default(),
                finish_reason: None,
                usage,
            })
        }
        Some("content_block_delta") => {
            let delta = &value["delta"];
            match delta.get("type").and_then(|v| v.as_str()) {
                Some("text_delta") => {
                    let text = delta["text"].as_str().unwrap_or("").to_string();
                    Some(ChatStreamChunk {
                        delta: ChatStreamDelta {
                            content: Some(text),
                            reasoning_content: None,
                            tool_calls: Vec::new(),
                        },
                        finish_reason: None,
                        usage: None,
                    })
                }
                Some("input_json_delta") => {
                    // Tool use argument streaming
                    let index = value["index"].as_u64().unwrap_or(0) as usize;
                    let partial_json = delta["partial_json"].as_str().unwrap_or("").to_string();
                    Some(ChatStreamChunk {
                        delta: ChatStreamDelta {
                            content: None,
                            reasoning_content: None,
                            tool_calls: vec![ToolCallDelta {
                                index,
                                id: None,
                                function_name: None,
                                function_arguments: Some(partial_json),
                            }],
                        },
                        finish_reason: None,
                        usage: None,
                    })
                }
                _ => None,
            }
        }
        Some("content_block_start") => {
            let block = &value["content_block"];
            match block.get("type").and_then(|v| v.as_str()) {
                Some("tool_use") => {
                    let index = value["index"].as_u64().unwrap_or(0) as usize;
                    let id = block["id"].as_str().unwrap_or("").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    Some(ChatStreamChunk {
                        delta: ChatStreamDelta {
                            content: None,
                            reasoning_content: None,
                            tool_calls: vec![ToolCallDelta {
                                index,
                                id: Some(id),
                                function_name: Some(name),
                                function_arguments: None,
                            }],
                        },
                        finish_reason: None,
                        usage: None,
                    })
                }
                _ => None,
            }
        }
        Some("message_delta") => {
            let delta = &value["delta"];
            let finish_reason = delta["stop_reason"].as_str().map(|r| match r {
                "end_turn" => FinishReason::Stop,
                "max_tokens" => FinishReason::Length,
                "tool_use" => FinishReason::ToolCalls,
                other => FinishReason::Other(other.to_string()),
            });

            let usage = value.get("usage").map(|u| crate::Usage {
                output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as usize,
                input_tokens: 0,
                total_tokens: u["output_tokens"].as_u64().unwrap_or(0) as usize,
            });

            Some(ChatStreamChunk {
                delta: ChatStreamDelta::default(),
                finish_reason,
                usage,
            })
        }
        Some("message_stop") => Some(ChatStreamChunk {
            delta: ChatStreamDelta::default(),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
        }),
        Some("ping") => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Anthropic API types (kept for reference / future use)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn test_provider_creation() {
        let provider = AnthropicProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_custom_model() {
        let provider =
            AnthropicProvider::new("test-key".to_string()).with_model("claude-opus-4".to_string());
        assert_eq!(provider.default_model(), "claude-opus-4");
    }

    #[test]
    fn test_build_request_basic() {
        let provider = AnthropicProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "claude-sonnet-4".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
            ],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            thinking: None,
        };

        let body = provider.build_request(request);

        // Check structure
        assert_eq!(body["model"], "claude-sonnet-4");
        assert_eq!(body["max_tokens"], 100);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["system"], "You are helpful");

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1); // only user message
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_build_request_with_tools() {
        let provider = AnthropicProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "claude-sonnet-4".to_string(),
            messages: vec![
                ChatMessage::user("What's the weather?"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new(
                        "tool_123",
                        "get_weather",
                        json!({"location": "NYC"}),
                    )],
                    None,
                ),
                ChatMessage::tool_result("tool_123", "get_weather", "Sunny, 72F"),
            ],
            tools: Some(vec![crate::ToolDefinition::function(
                "get_weather",
                "Get weather info",
                json!({"type": "object", "properties": {}}),
            )]),
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let body = provider.build_request(request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // Assistant message with tool_use
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "get_weather");

        // Tool result message → user with tool_result block
        let tool_result = &messages[2];
        assert_eq!(tool_result["role"], "user");
        let tr_content = tool_result["content"].as_array().unwrap();
        assert_eq!(tr_content[0]["type"], "tool_result");
        assert_eq!(tr_content[0]["tool_use_id"], "tool_123");
    }

    #[test]
    fn test_parse_response() {
        let provider = AnthropicProvider::new("test-key".to_string());

        let response = json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.content, Some("Hello, world!".to_string()));
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_some());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_response_with_tool_use() {
        let provider = AnthropicProvider::new("test-key".to_string());

        let response = json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_123", "name": "read_file", "input": {"path": "test.txt"}}
            ]
        });

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.content, None);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].function.name, "read_file");
        assert_eq!(result.tool_calls[0].id, "toolu_123");
    }
}
