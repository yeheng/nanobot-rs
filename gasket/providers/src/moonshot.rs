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
//! # API Endpoint
//!
//! `POST {api_base}/chat/completions`

use crate::common::build_http_client;
use crate::streaming::parse_sse_stream;
use crate::{ChatRequest, ChatResponse, ChatStream, LlmProvider, ToolCall};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
// serde::Deserialize is used via derive macros
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, instrument};

/// Default API base for Moonshot
const MOONSHOT_API_BASE: &str = "https://api.moonshot.cn/v1";

/// Default model for Moonshot
const DEFAULT_MODEL: &str = "kimi-k2.5";

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

    /// Build Moonshot request body (OpenAI-compatible with extensions)
    fn build_request(&self, request: ChatRequest) -> Value {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model
        };

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

                // Name (for role-play consistency)
                if let Some(name) = msg.name {
                    obj.insert("name".to_string(), json!(name));
                }

                // Tool calls
                if let Some(tool_calls) = msg.tool_calls {
                    let tc: Vec<Value> = tool_calls
                        .into_iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": tc.tool_type,
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
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
            "Moonshot request: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<serialization error>".to_string())
        );
        body
    }

    /// Parse Moonshot response
    fn parse_response(&self, response: Value) -> Result<ChatResponse, crate::ProviderError> {
        debug!(
            "Moonshot response: {}",
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
            reasoning_content: None,
            usage,
        })
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
        let url = format!("{}/chat/completions", self.api_base);
        let mut body = self.build_request(request);
        body["stream"] = json!(false);

        debug!("[moonshot] POST {}", url);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.extra_headers {
            req = req.header(key, value);
        }

        let response = req.json(&body).send().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Moonshot request failed: {}", e))
        })?;

        let status = response.status();
        debug!("[moonshot] response status: {}", status);

        let response_text = response.text().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Failed to read Moonshot response: {}", e))
        })?;
        debug!("[moonshot] response body:\n{}", response_text);

        if !status.is_success() {
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

        self.parse_response(response_value)
    }

    #[instrument(skip(self, request), fields(provider = "moonshot", model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::ProviderError> {
        let url = format!("{}/chat/completions", self.api_base);
        let mut body = self.build_request(request);
        body["stream"] = json!(true);

        debug!("[moonshot] POST {} (stream)", url);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.extra_headers {
            req = req.header(key, value);
        }

        let response = req.json(&body).send().await.map_err(|e| {
            crate::ProviderError::NetworkError(format!("Moonshot stream request failed: {}", e))
        })?;

        let status = response.status();
        debug!("[moonshot] stream response status: {}", status);

        if !status.is_success() {
            let body = response.text().await.map_err(|e| {
                crate::ProviderError::NetworkError(format!(
                    "Failed to read Moonshot stream response: {}",
                    e
                ))
            })?;
            return Err(crate::ProviderError::ApiError {
                status_code: status.as_u16(),
                message: format!("{} - {}", status, body),
            });
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(chunk_stream))
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
    }

    #[test]
    fn test_custom_model() {
        let provider =
            MoonshotProvider::new("test-key".to_string()).with_model("kimi-k2".to_string());
        assert_eq!(provider.default_model(), "kimi-k2");
    }

    #[test]
    fn test_build_request_basic() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "kimi-k2.5".to_string(),
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

        assert_eq!(body["model"], "kimi-k2.5");
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["max_tokens"], 100);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_build_request_with_tools() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let request = ChatRequest {
            model: "kimi-k2.5".to_string(),
            messages: vec![
                ChatMessage::user("What's the weather?"),
                ChatMessage::assistant_with_tools(
                    None,
                    vec![ToolCall::new(
                        "call_123",
                        "get_weather",
                        serde_json::json!({"location": "NYC"}),
                    )],
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

        let body = provider.build_request(request);
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
    fn test_parse_response() {
        let provider = MoonshotProvider::new("test-key".to_string());

        let response = serde_json::json!({
            "id": "cmpl-123",
            "object": "chat.completion",
            "created": 1698999496,
            "model": "kimi-k2.5",
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

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.content, Some("Hello, world!".to_string()));
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_some());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 13); // 10 + 3 cached
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.total_tokens, 18); // 10 + 5 + 3 cached
    }

    #[test]
    fn test_parse_response_with_tool_calls() {
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

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.content, None);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].function.name, "read_file");
        assert_eq!(result.tool_calls[0].id, "call_123");
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
            model: "kimi-k2.5".to_string(),
            messages: vec![ChatMessage::user("Hello")],
            tools: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
        };

        let body = provider.build_request(request);
        assert_eq!(body["cache_tag"], "session-123");
        assert_eq!(body["user_id"], "user-456");
    }
}
