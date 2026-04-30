//! Moonshot (Kimi) LLM provider
//!
//! Implements the native Moonshot API which is OpenAI-compatible with vendor-specific
//! extensions:
//!
//! - **Context Caching**: `cache_tag` for cache hit optimization
//! - **Partial Mode**: `partial: true` on assistant messages for output prefill
//! - **Multimodal content**: `content` can be an array of `text` / `image_url` / `video_url` blocks
//! - **Thinking mode**: `thinking` config for kimi-k2.6 models
//! - **Cached tokens**: `cached_tokens` in usage response
//!
//! Also supports Anthropic-format endpoints (`/coding`, `/anthropic`) which use the
//! Anthropic Messages API instead of the OpenAI-compatible endpoint.
//!
//! # API Endpoint
//!
//! `POST {api_base}/chat/completions`  (OpenAI format, api_base ends with `/v1`)
//! `POST {api_base}/messages`          (Anthropic format, api_base ends with `/coding` or `/anthropic`)

use crate::base::{ChatStreamChunk, ChatStreamDelta, FinishReason, ToolCallDelta};
use crate::common::build_http_client;
use crate::streaming::{parse_sse_stream, sse_lines};
use crate::{ChatRequest, ChatResponse, ChatStream, LlmProvider, ToolCall};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::StreamExt;
use reqwest::Client;
// serde::Deserialize is used via derive macros
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, error, info, instrument, warn};

/// Default API base for Moonshot
const MOONSHOT_API_BASE: &str = "https://api.moonshot.cn/v1";

/// Default model for Moonshot
const DEFAULT_MODEL: &str = "kimi-k2.6";

/// Default max tokens for Anthropic-format endpoints (required parameter)
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic API version header (used for Anthropic-format endpoints)
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// API format detected from the api_base URL suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFormat {
    /// OpenAI-compatible format (`/v1`)
    OpenAI,
    /// Anthropic Messages API format (`/coding`, `/anthropic`)
    Anthropic,
}

/// Moonshot provider
pub struct MoonshotProvider {
    /// HTTP client
    client: Client,

    /// API key
    api_key: String,

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
    /// end with `/v1`, we append `/v1` automatically. Moonshot's coding and
    /// anthropic-compatible endpoints require the `/v1` path segment before
    /// the final resource path (e.g. `/v1/messages`).
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
        Self {
            client: build_http_client(None, None, None),
            api_key,
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
        Self {
            client: build_http_client(
                proxy_url.as_deref(),
                proxy_username.as_deref(),
                proxy_password.as_deref(),
            ),
            api_key,
            api_base: MOONSHOT_API_BASE.to_string(),
            default_model: DEFAULT_MODEL.to_string(),
            default_cache_tag: None,
            default_user_id: None,
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
        Self {
            client: build_http_client(
                proxy_url.as_deref(),
                proxy_username.as_deref(),
                proxy_password.as_deref(),
            ),
            api_key,
            api_base: api_base.unwrap_or_else(|| MOONSHOT_API_BASE.to_string()),
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

    // -----------------------------------------------------------------------
    // OpenAI-format request/response
    // -----------------------------------------------------------------------

    /// Build Moonshot request body (OpenAI-compatible with extensions)
    fn build_openai_request(&self, request: ChatRequest) -> Value {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model
        };

        // When thinking is enabled, Moonshot requires reasoning_content in all
        // assistant tool call messages — even if the model didn't produce reasoning.
        let thinking_enabled = request
            .thinking
            .as_ref()
            .is_some_and(|t| t.thinking_type == "enabled");

        let messages: Vec<Value> = request
            .messages
            .into_iter()
            .map(|msg| {
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), json!(msg.role.as_str()));

                // Content: string or array for multimodal
                if let Some(content) = msg.content {
                    obj.insert("content".to_string(), json!(content));
                }

                // Reasoning content (for thinking-enabled models like K2.5/K2.6)
                let has_nonempty_reasoning = msg
                    .reasoning_content
                    .as_ref()
                    .is_some_and(|r| !r.is_empty());
                if has_nonempty_reasoning {
                    obj.insert(
                        "reasoning_content".to_string(),
                        json!(msg.reasoning_content.unwrap()),
                    );
                } else if thinking_enabled
                    && matches!(msg.role, crate::MessageRole::Assistant)
                    && msg.tool_calls.is_some()
                {
                    // Moonshot requires reasoning_content in assistant tool call
                    // messages when thinking is enabled. Inject empty string if
                    // the model didn't produce reasoning before the tool call.
                    obj.insert("reasoning_content".to_string(), json!(""));
                }

                // Name (for role-play consistency)
                if let Some(name) = msg.name {
                    obj.insert("name".to_string(), json!(name));
                }

                // Tool calls
                if let Some(tool_calls) = msg.tool_calls {
                    let tc: Vec<Value> = tool_calls
                        .into_iter()
                        .map(|tc| {
                            let args_str = serde_json::to_string(&tc.function.arguments)
                                .unwrap_or_else(|_| "{}".to_string());
                            json!({
                                "id": tc.id,
                                "type": tc.tool_type,
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": args_str,
                                }
                            })
                        })
                        .collect();
                    obj.insert("tool_calls".to_string(), json!(tc));
                }

                // Tool call ID
                if let Some(tool_call_id) = msg.tool_call_id {
                    obj.insert("tool_call_id".to_string(), json!(tool_call_id));
                }

                Value::Object(obj)
            })
            .collect();

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

        // Tools
        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
            }
        }

        // Thinking config (for kimi-k2.6)
        if let Some(thinking) = request.thinking {
            body["thinking"] = json!({"type": thinking.thinking_type});
        }

        // Cache tag for context caching
        if let Some(ref cache_tag) = self.default_cache_tag {
            body["cache_tag"] = json!(cache_tag);
        }

        // User ID
        if let Some(ref user_id) = self.default_user_id {
            body["user_id"] = json!(user_id);
        }

        debug!(
            "Moonshot OpenAI request: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse Moonshot response (OpenAI format)
    fn parse_openai_response(&self, response: Value) -> Result<ChatResponse, crate::ProviderError> {
        debug!(
            "Moonshot OpenAI response: {}",
            serde_json::to_string(&response)
                .unwrap_or_else(|_| "<serialization error>".to_string())
        );

        // Check for errors
        if let Some(error) = response.get("error") {
            return Err(crate::ProviderError::ApiError {
                status_code: error["code"]
                    .as_str()
                    .and_then(|c| c.parse::<u16>().ok())
                    .unwrap_or(400),
                message: format!("Moonshot API error: {}", error),
            });
        }

        let choices = response["choices"].as_array().ok_or_else(|| {
            crate::ProviderError::ParseError("No choices in Moonshot response".to_string())
        })?;

        if choices.is_empty() {
            return Err(crate::ProviderError::ParseError(
                "Empty choices in Moonshot response".to_string(),
            ));
        }

        let first = &choices[0];
        let message = &first["message"];

        let content = message["content"].as_str().map(|s| s.to_string());
        let reasoning_content = message["reasoning_content"].as_str().map(|s| s.to_string());

        let tool_calls: Vec<ToolCall> = message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tc| {
                let id = tc["id"].as_str().unwrap_or("").to_string();
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                let args = tc["function"]
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| {
                        tc["function"]
                            .get("arguments")
                            .cloned()
                            .unwrap_or(json!({}))
                    });
                ToolCall::new(id, name, args)
            })
            .collect();

        // Parse usage with cached_tokens extension
        let usage = response.get("usage").map(|u| {
            let input = u["prompt_tokens"].as_u64().unwrap_or(0) as usize;
            let output = u["completion_tokens"].as_u64().unwrap_or(0) as usize;
            let cached = u["cached_tokens"].as_u64().unwrap_or(0) as usize;
            crate::Usage {
                input_tokens: input + cached,
                output_tokens: output,
                total_tokens: input + output + cached,
            }
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            reasoning_content,
            usage,
            finish_reason: None,
        })
    }

    // -----------------------------------------------------------------------
    // Anthropic-format request/response
    // -----------------------------------------------------------------------

    /// Build Anthropic Messages API request body.
    fn build_anthropic_request(&self, request: ChatRequest) -> Value {
        let mut messages = Vec::new();
        let mut system_instruction = None;

        let thinking_enabled = request
            .thinking
            .as_ref()
            .is_some_and(|t| t.thinking_type == "enabled");

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

                    // Add thinking/reasoning content if present (required for thinking-enabled models)
                    let has_nonempty_reasoning = msg
                        .reasoning_content
                        .as_ref()
                        .is_some_and(|r| !r.is_empty());
                    if has_nonempty_reasoning {
                        content_blocks.push(json!({
                            "type": "thinking",
                            "thinking": msg.reasoning_content.as_ref().unwrap(),
                        }));
                    } else if thinking_enabled && msg.tool_calls.is_some() {
                        // Moonshot requires thinking content in assistant tool call
                        // messages when thinking is enabled.
                        content_blocks.push(json!({
                            "type": "thinking",
                            "thinking": "",
                        }));
                    }

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
            "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
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

        // Thinking mode
        if let Some(thinking) = request.thinking {
            body["thinking"] = json!({"type": thinking.thinking_type});
        }

        debug!(
            "Moonshot Anthropic request: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse Anthropic response into ChatResponse.
    fn parse_anthropic_response(
        &self,
        response: Value,
    ) -> Result<ChatResponse, crate::ProviderError> {
        debug!(
            "Moonshot Anthropic response: {}",
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
                message: format!("Moonshot Anthropic API error: {}", error),
            });
        }

        let content_blocks = response["content"].as_array().cloned().unwrap_or_default();

        let mut text_parts = Vec::new();
        let mut reasoning_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in &content_blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        text_parts.push(text.to_string());
                    }
                }
                Some("thinking") => {
                    if let Some(thinking) = block.get("thinking").and_then(|v| v.as_str()) {
                        reasoning_parts.push(thinking.to_string());
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

        let reasoning_content = if reasoning_parts.is_empty() {
            None
        } else {
            Some(reasoning_parts.join(""))
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
            reasoning_content,
            usage,
            finish_reason: None,
        })
    }

    /// Build headers for Anthropic-format API requests.
    fn build_anthropic_headers(&self) -> reqwest::header::HeaderMap {
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

        let (url, body) = match format {
            ApiFormat::OpenAI => {
                let url = format!("{}/chat/completions", self.resolved_api_base());
                let mut body = self.build_openai_request(request);
                body["stream"] = json!(false);
                (url, body)
            }
            ApiFormat::Anthropic => {
                let url = format!("{}/messages", self.resolved_api_base());
                let body = self.build_anthropic_request(request);
                (url, body)
            }
        };

        info!("[moonshot] POST {}", url);

        let response = match format {
            ApiFormat::OpenAI => {
                let mut req = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json");

                for (key, value) in &self.extra_headers {
                    req = req.header(key, value);
                }

                req.json(&body).send().await.map_err(|e| {
                    crate::ProviderError::NetworkError(format!("Moonshot request failed: {}", e))
                })?
            }
            ApiFormat::Anthropic => self
                .client
                .post(&url)
                .headers(self.build_anthropic_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    crate::ProviderError::NetworkError(format!(
                        "Moonshot Anthropic request failed: {}",
                        e
                    ))
                })?,
        };

        let status = response.status();
        info!("[moonshot] response status: {}", status);

        let response_text = response.text().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Failed to read Moonshot response: {}", e))
        })?;
        debug!("[moonshot] response body:\n{}", response_text);

        if !status.is_success() {
            error!("[moonshot] error: {} | body: {}", status, response_text);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, response_text),
            });
        }

        let response_value: Value = serde_json::from_str(&response_text).map_err(|e| {
            crate::ProviderError::ParseError(format!(
                "Moonshot API response parse error: {} | body: {}",
                e, response_text
            ))
        })?;

        match format {
            ApiFormat::OpenAI => self.parse_openai_response(response_value),
            ApiFormat::Anthropic => self.parse_anthropic_response(response_value),
        }
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let format = self.api_format();

        let (url, body) = match format {
            ApiFormat::OpenAI => {
                let url = format!("{}/chat/completions", self.resolved_api_base());
                let mut body = self.build_openai_request(request);
                body["stream"] = json!(true);
                (url, body)
            }
            ApiFormat::Anthropic => {
                let url = format!("{}/messages", self.resolved_api_base());
                let mut body = self.build_anthropic_request(request);
                body["stream"] = json!(true);
                (url, body)
            }
        };

        info!("[moonshot] {:?} POST {}", format, url);

        let response = match format {
            ApiFormat::OpenAI => {
                let mut req = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json");

                for (key, value) in &self.extra_headers {
                    req = req.header(key, value);
                }

                req.json(&body).send().await.map_err(|e| {
                    crate::ProviderError::NetworkError(format!(
                        "Moonshot stream request failed: {}",
                        e
                    ))
                })?
            }
            ApiFormat::Anthropic => self
                .client
                .post(&url)
                .headers(self.build_anthropic_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    crate::ProviderError::NetworkError(format!(
                        "Moonshot Anthropic stream request failed: {}",
                        e
                    ))
                })?,
        };

        let status = response.status();
        info!("[moonshot] stream response status: {}", status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!(
                    "Failed to read Moonshot stream response: {}",
                    e
                ))
            })?;
            error!("[moonshot] stream error: {} | body: {}", status, body);
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, body),
            });
        }

        let byte_stream = response.bytes_stream();

        match format {
            ApiFormat::OpenAI => {
                let chunk_stream = parse_sse_stream(byte_stream);
                Ok(Box::pin(chunk_stream))
            }
            ApiFormat::Anthropic => {
                let chunk_stream = parse_anthropic_sse_stream(byte_stream);
                Ok(Box::pin(chunk_stream))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Anthropic SSE stream parsing (for Anthropic-format endpoints)
// ---------------------------------------------------------------------------

/// Parse an Anthropic SSE byte stream into `ChatStreamChunk`s.
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

                if let Some(ev) = line.strip_prefix("event:").map(|s| s.trim_start()) {
                    event_type = Some(ev.to_string());
                    continue;
                }

                if let Some(data) = line.strip_prefix("data:").map(|s| s.trim_start()) {
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
                Some("thinking_delta") => {
                    let thinking = delta["thinking"].as_str().unwrap_or("").to_string();
                    Some(ChatStreamChunk {
                        delta: ChatStreamDelta {
                            content: None,
                            reasoning_content: Some(thinking),
                            tool_calls: Vec::new(),
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
        Some("content_block_stop") => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

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
        let provider =
            MoonshotProvider::new("test-key".to_string()).with_model("kimi-k2".to_string());
        assert_eq!(provider.default_model(), "kimi-k2");
    }

    #[test]
    fn test_build_openai_request_basic() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
            ],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            thinking: None,
        };

        let body = provider.build_openai_request(request);

        assert_eq!(body["model"], "kimi-k2.6");
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["max_tokens"], 100);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_build_openai_request_with_tools() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::user("What's the weather?"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new(
                        "call_123",
                        "get_weather",
                        serde_json::json!({"location": "NYC"}),
                    )],
                    None,
                ),
                ChatMessage::tool_result("call_123", "get_weather", "Sunny, 72F"),
            ],
            tools: Some(vec![crate::ToolDefinition::function(
                "get_weather",
                "Get weather info",
                serde_json::json!({"type": "object", "properties": {}}),
            )]),
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let body = provider.build_openai_request(request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // Assistant message with tool_calls
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        assert!(assistant["tool_calls"].is_array());

        // Tool result message
        let tool_result = &messages[2];
        assert_eq!(tool_result["role"], "tool");
        assert_eq!(tool_result["tool_call_id"], "call_123");
    }

    #[test]
    fn test_build_openai_request_with_tools_thinking_enabled_no_reasoning() {
        let provider = MoonshotProvider::new("test-key".to_string());

        // Simulate a multi-step execution where the model produced a tool call
        // without reasoning_content (None).
        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::user("Search for X"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new(
                        "call_fn_abc_1",
                        "web_search",
                        json!({"query": "X"}),
                    )],
                    None, // No reasoning_content
                ),
                ChatMessage::tool_result("call_fn_abc_1", "web_search", "Result"),
            ],
            tools: Some(vec![crate::ToolDefinition::function(
                "web_search",
                "Search",
                json!({"type": "object", "properties": {}}),
            )]),
            temperature: None,
            max_tokens: None,
            thinking: Some(crate::ThinkingConfig::enabled()),
        };

        let body = provider.build_openai_request(request);
        let messages = body["messages"].as_array().unwrap();

        // Assistant tool call message must have reasoning_content injected
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["reasoning_content"], "");
        assert!(assistant["tool_calls"].is_array());
    }

    #[test]
    fn test_build_openai_request_with_tools_thinking_enabled_empty_reasoning() {
        let provider = MoonshotProvider::new("test-key".to_string());

        // Edge case: reasoning_content is Some("") (empty string)
        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::user("Search for X"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new("call_1", "search", json!({}))],
                    Some(String::new()), // Empty reasoning
                ),
                ChatMessage::tool_result("call_1", "search", "Result"),
            ],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: Some(crate::ThinkingConfig::enabled()),
        };

        let body = provider.build_openai_request(request);
        let messages = body["messages"].as_array().unwrap();

        // Even with Some(""), reasoning_content must be injected as ""
        let assistant = &messages[1];
        assert_eq!(assistant["reasoning_content"], "");
    }

    #[test]
    fn test_parse_openai_response() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let response = serde_json::json!({
            "id": "cmpl-123",
            "object": "chat.completion",
            "created": 1698999496,
            "model": "kimi-k2.6",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello, world!"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "cached_tokens": 3
            }
        });

        let result = provider.parse_openai_response(response).unwrap();
        assert_eq!(result.content, Some("Hello, world!".to_string()));
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_some());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 13); // 10 + 3 cached
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.total_tokens, 18); // 10 + 5 + 3 cached
    }

    #[test]
    fn test_parse_openai_response_with_tool_calls() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let response = serde_json::json!({
            "id": "cmpl-123",
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
        });

        let result = provider.parse_openai_response(response).unwrap();
        assert_eq!(result.content, None);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].function.name, "read_file");
        assert_eq!(result.tool_calls[0].id, "call_123");
    }

    #[test]
    fn test_build_anthropic_request_basic() {
        let provider = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/anthropic".to_string(),
        );

        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
            ],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            thinking: None,
        };

        let body = provider.build_anthropic_request(request);

        assert_eq!(body["model"], "kimi-k2.6");
        assert_eq!(body["max_tokens"], 100);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["system"], "You are helpful");

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1); // only user message
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_build_anthropic_request_with_tools() {
        let provider = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/anthropic".to_string(),
        );

        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![
                ChatMessage::user("What's the weather?"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new(
                        "tool_123",
                        "get_weather",
                        serde_json::json!({"location": "NYC"}),
                    )],
                    None,
                ),
                ChatMessage::tool_result("tool_123", "get_weather", "Sunny, 72F"),
            ],
            tools: Some(vec![crate::ToolDefinition::function(
                "get_weather",
                "Get weather info",
                serde_json::json!({"type": "object", "properties": {}}),
            )]),
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let body = provider.build_anthropic_request(request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // Assistant message with tool_use
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "get_weather");

        // Tool result message -> user with tool_result block
        let tool_result = &messages[2];
        assert_eq!(tool_result["role"], "user");
        let tr_content = tool_result["content"].as_array().unwrap();
        assert_eq!(tr_content[0]["type"], "tool_result");
        assert_eq!(tr_content[0]["tool_use_id"], "tool_123");
    }

    #[test]
    fn test_parse_anthropic_response() {
        let provider = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/anthropic".to_string(),
        );

        let response = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let result = provider.parse_anthropic_response(response).unwrap();
        assert_eq!(result.content, Some("Hello, world!".to_string()));
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_some());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_anthropic_response_with_tool_use() {
        let provider = MoonshotProvider::with_api_base(
            "test-key".to_string(),
            "https://api.moonshot.cn/anthropic".to_string(),
        );

        let response = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_123", "name": "read_file", "input": {"path": "test.txt"}}
            ]
        });

        let result = provider.parse_anthropic_response(response).unwrap();
        assert_eq!(result.content, None);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].function.name, "read_file");
        assert_eq!(result.tool_calls[0].id, "toolu_123");
    }

    #[test]
    fn test_cache_tag_and_user_id() {
        let provider = MoonshotProvider::with_config(
            "test-key".to_string(),
            None,
            None,
            Some("session-123".to_string()),
            Some("user-456".to_string()),
            None,
            None,
            None,
            std::collections::HashMap::new(),
        );

        let request = ChatRequest {
            model: "kimi-k2.6".to_string(),
            messages: vec![ChatMessage::user("Hello")],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let body = provider.build_openai_request(request);
        assert_eq!(body["cache_tag"], "session-123");
        assert_eq!(body["user_id"], "user-456");
    }
}
