//! MiniMax LLM provider
//!
//! Implements the MiniMax native API with OpenAI-compatible format.
//!
//! # API Notes
//!
//! - Endpoint: `POST /v1/chat/completions` (OpenAI-compatible)
//! - Auth: `Authorization: Bearer` header
//! - Thinking content is returned in `reasoning_details` field when `reasoning_split=true`
//! - Streaming uses standard SSE with MiniMax-specific fields

use crate::base::{ChatStream, ChatStreamChunk, ChatStreamDelta, FinishReason, ToolCallDelta};
use crate::common::build_http_client;
use crate::streaming::sse_lines;
use crate::{ChatRequest, ChatResponse, LlmProvider, ToolCall};
use async_trait::async_trait;
use futures_util::stream::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, instrument, warn};

/// Default API base for MiniMax
const MINIMAX_API_BASE: &str = "https://api.minimaxi.com/v1";

/// Merge consecutive messages with the same role.
///
/// MiniMax API rejects multiple consecutive messages with the same role (error 2013).
/// This merges their content with a double newline separator.
fn merge_consecutive_messages(messages: Vec<crate::ChatMessage>) -> Vec<crate::ChatMessage> {
    let mut merged: Vec<crate::ChatMessage> = Vec::new();
    for msg in messages {
        if let Some(last) = merged.last_mut() {
            if last.role == msg.role {
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

/// Default model for MiniMax
const DEFAULT_MODEL: &str = "MiniMax-M2.7";

/// MiniMax provider using the native API
pub struct MinimaxProvider {
    /// HTTP client
    client: Client,

    /// API key
    api_key: String,

    /// API base URL
    api_base: String,

    /// Default model
    default_model: String,

    /// Group ID for MiniMax API
    group_id: Option<String>,

    /// Extra HTTP headers to send with every request
    extra_headers: HashMap<String, String>,
}

impl MinimaxProvider {
    /// Create a new Minimax provider
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_http_client(None, None, None),
            api_key,
            api_base: MINIMAX_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            group_id: None,
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
            api_base: MINIMAX_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            group_id: None,
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
            group_id: None,
            extra_headers: HashMap::new(),
        }
    }

    /// Create with group_id for Multi-account API access
    pub fn with_group_id(mut self, group_id: String) -> Self {
        self.group_id = Some(group_id);
        self
    }

    /// Create with full configuration
    #[allow(clippy::too_many_arguments)]
    pub fn with_config(
        api_key: String,
        api_base: Option<String>,
        default_model: Option<String>,
        group_id: Option<String>,
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
            api_base: api_base.unwrap_or_else(|| MINIMAX_API_BASE.to_string()),
            default_model: default_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            group_id,
            extra_headers,
        }
    }

    /// Set default model
    pub fn with_model(mut self, model: String) -> Self {
        self.default_model = model;
        self
    }

    /// Build MiniMax request body
    fn build_request(&self, request: ChatRequest) -> Value {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        // MiniMax API rejects multiple consecutive messages with the same role (error 2013).
        // Merge consecutive same-role messages to work around this limitation.
        let messages = merge_consecutive_messages(request.messages);

        let mut body = json!({
            "model": model,
            "messages": messages,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        // Add tools if present
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
            }
        }

        // Enable reasoning split to get thinking content separately
        body["reasoning_split"] = json!(true);

        debug!(
            "MiniMax request: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse MiniMax response into ChatResponse
    fn parse_response(&self, response: Value) -> Result<ChatResponse, crate::ProviderError> {
        debug!(
            "MiniMax response: {}",
            serde_json::to_string(&response)
                .unwrap_or_else(|_| "<serialization error>".to_string())
        );

        let choices = response["choices"].as_array().cloned().unwrap_or_default();

        let choice = choices.into_iter().next().ok_or_else(|| {
            crate::ProviderError::ParseError("No choices in MiniMax response".to_string())
        })?;

        let message = choice["message"].clone();

        // Extract reasoning content from reasoning_details if present
        let reasoning_content = message["reasoning_details"].as_array().and_then(|details| {
            let texts: Vec<String> = details
                .iter()
                .filter_map(|d| d["text"].as_str().map(String::from))
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(""))
            }
        });

        let content = message["content"].as_str().map(String::from);

        // Extract tool calls
        let tool_calls = message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|tc| {
                let id = tc["id"].as_str()?.to_string();
                let name = tc["function"]["name"].as_str()?.to_string();
                let arguments = tc["function"]["arguments"].clone();
                Some(ToolCall::new(id, name, arguments))
            })
            .collect();

        let usage = response["usage"].as_object().map(|u| crate::Usage {
            input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            reasoning_content,
            usage,
        })
    }

    /// Build headers for MiniMax API requests
    fn build_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::HeaderMap;

        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.api_key).parse().unwrap(),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        if let Some(ref group_id) = self.group_id {
            headers.insert("X-Group-Id", group_id.parse().unwrap());
        }

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
impl LlmProvider for MinimaxProvider {
    fn name(&self) -> &str {
        "minimax"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request), fields(provider = "minimax", model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::ProviderError> {
        let url = format!("{}/chat/completions", self.api_base);
        let body = self.build_request(request);

        debug!("[minimax] POST {}", url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::ProviderError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        debug!("[minimax] response status: {}", status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!("Failed to read error body: {}", e))
            })?;
            debug!("[minimax] error response: {}", body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Failed to read response: {}", e))
        })?;

        let json: Value = serde_json::from_str(&body).map_err(|e| {
            crate::ProviderError::ParseError(format!("Failed to parse response: {}", e))
        })?;

        self.parse_response(json)
    }

    #[instrument(skip(self, request), fields(provider = "minimax", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let url = format!("{}/chat/completions", self.api_base);
        let mut body = self.build_request(request);
        body["stream"] = json!(true);

        debug!("[minimax] POST {} (stream)", url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::ProviderError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        debug!("[minimax] stream response status: {}", status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!("Failed to read error body: {}", e))
            })?;
            debug!("[minimax] error response: {}", body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let lines_stream = sse_lines(byte_stream);

        let chunk_stream = lines_stream.filter_map(|line_result| async move {
            match line_result {
                Err(e) => Some(Err(crate::ProviderError::NetworkError(format!(
                    "SSE stream error: {}",
                    e
                )))),
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with(':') {
                        return None;
                    }

                    let data = line.strip_prefix("data: ")?;

                    if data.trim() == "[DONE]" {
                        return None;
                    }

                    match serde_json::from_str::<Value>(data) {
                        Ok(value) => Some(Ok(parse_minimax_stream_chunk(value))),
                        Err(e) => {
                            warn!("Failed to parse MiniMax SSE chunk: {} | data: {}", e, data);
                            None
                        }
                    }
                }
            }
        });

        Ok(Box::pin(chunk_stream))
    }
}

/// Parse a MiniMax streaming chunk
fn parse_minimax_stream_chunk(value: Value) -> ChatStreamChunk {
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

    // Extract reasoning content from reasoning_details
    let reasoning_content = delta["reasoning_details"].as_array().and_then(|details| {
        let texts: Vec<String> = details
            .iter()
            .filter_map(|d| d["text"].as_str().map(String::from))
            .collect();
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(""))
        }
    });

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn test_provider_creation() {
        let provider = MinimaxProvider::new("test-key".to_string());
        assert_eq!(provider.name(), "minimax");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn test_custom_model() {
        let provider =
            MinimaxProvider::new("test-key".to_string()).with_model("MiniMax-M2.5".to_string());
        assert_eq!(provider.default_model(), "MiniMax-M2.5");
    }

    #[test]
    fn test_build_request_basic() {
        let provider = MinimaxProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "MiniMax-M2.7".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
            ],
            tools: None,
            temperature: Some(1.0),
            max_tokens: Some(1000),
            thinking: None,
        };

        let body = provider.build_request(request);

        assert_eq!(body["model"], "MiniMax-M2.7");
        assert_eq!(body["temperature"], 1.0);
        assert_eq!(body["max_tokens"], 1000);
        assert!(body["reasoning_split"].as_bool().unwrap());
    }

    #[test]
    fn test_merge_consecutive_system_messages() {
        let provider = MinimaxProvider::new("test-key".to_string());

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

        let body = provider.build_request(request);
        let msgs = body["messages"].as_array().unwrap();

        // Two consecutive system messages should be merged into one
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "System prompt 1\n\nSystem prompt 2");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "Hello\n\nWorld");
    }

    #[test]
    fn test_parse_response() {
        let provider = MinimaxProvider::new("test-key".to_string());

        let response = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "MiniMax-M2.7",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!",
                    "reasoning_details": [{"text": "Let me think..."}]
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.content, Some("Hello!".to_string()));
        assert_eq!(
            result.reasoning_content,
            Some("Let me think...".to_string())
        );
    }

    #[test]
    fn test_parse_streaming_chunk() {
        let chunk = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1234567890,
            "model": "MiniMax-M2.7",
            "choices": [{
                "index": 0,
                "delta": {
                    "content": "Hello",
                    "reasoning_details": [{"text": "Thinking..."}]
                },
                "finish_reason": null
            }]
        });

        let result = parse_minimax_stream_chunk(chunk);
        assert_eq!(result.delta.content, Some("Hello".to_string()));
        assert_eq!(
            result.delta.reasoning_content,
            Some("Thinking...".to_string())
        );
    }
}
