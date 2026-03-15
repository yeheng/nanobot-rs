//! Base traits and types for LLM providers

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::{self, Stream};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Type alias for a boxed stream of chat stream chunks
pub type ChatStream = Pin<Box<dyn Stream<Item = anyhow::Result<ChatStreamChunk>> + Send>>;

/// LLM Provider trait
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get the provider name
    fn name(&self) -> &str;

    /// Get the default model for this provider
    fn default_model(&self) -> &str;

    /// Send a chat completion request
    ///
    /// Observability is handled automatically via the `tracing` crate's
    /// implicit span context — no manual context passing needed.
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;

    /// Send a streaming chat completion request.
    ///
    /// The default implementation falls back to `chat()` and wraps the
    /// complete response in a single-chunk stream.
    async fn chat_stream(&self, request: ChatRequest) -> anyhow::Result<ChatStream> {
        let response = self.chat(request).await?;
        Ok(Box::pin(stream::once(async move {
            Ok(ChatStreamChunk::from_response(response))
        })))
    }
}

/// Chat completion request
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    /// Model to use
    pub model: String,

    /// Messages in the conversation
    pub messages: Vec<ChatMessage>,

    /// Available tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    /// Temperature for generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum tokens to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Thinking configuration for deep reasoning mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

/// Role of the message sender
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for MessageRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            _ => Err(format!("Unknown role: {}", s)),
        }
    }
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role: system, user, assistant, or tool
    pub role: MessageRole,

    /// Message content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Tool calls (for assistant messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    /// Tool call ID (for tool response messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Name (for tool response messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create an assistant message with tool calls
    pub fn assistant_with_tools(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    /// Create a tool response message
    pub fn tool_result(
        id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(id.into()),
            name: Some(name.into()),
        }
    }
}

/// Tool definition for LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (always "function" for now)
    #[serde(rename = "type")]
    pub tool_type: String,

    /// Function definition
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// Create a new function tool definition
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// Function definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name
    pub name: String,

    /// Function description
    pub description: String,

    /// JSON Schema for parameters
    pub parameters: serde_json::Value,
}

/// Thinking configuration for LLM deep reasoning mode
///
/// This is a generic configuration that works with any model that supports
/// thinking/reasoning mode (e.g., GLM-5, DeepSeek R1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Type of thinking mode: "enabled" or "disabled"
    #[serde(rename = "type")]
    pub thinking_type: String,
}

impl ThinkingConfig {
    /// Create an enabled thinking config
    pub fn enabled() -> Self {
        Self {
            thinking_type: "enabled".to_string(),
        }
    }

    /// Create a disabled thinking config
    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self {
            thinking_type: "disabled".to_string(),
        }
    }

    /// Check if thinking is enabled
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.thinking_type == "enabled"
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Tool call from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID
    pub id: String,

    /// Tool type
    #[serde(rename = "type")]
    pub tool_type: String,

    /// Function call details
    pub function: FunctionCall,
}

impl ToolCall {
    /// Create a new tool call
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments,
            },
        }
    }
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name
    pub name: String,

    /// Function arguments — stored as `serde_json::Value` for internal use,
    /// but serialized as a JSON **string** (as required by OpenAI-compatible APIs).
    #[serde(
        serialize_with = "serialize_args_as_string",
        deserialize_with = "deserialize_args_from_string_or_object"
    )]
    pub arguments: serde_json::Value,
}

/// Serialize `serde_json::Value` as a JSON string (e.g. `"{\"path\": \".\"}"`)
fn serialize_args_as_string<S>(value: &serde_json::Value, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = serde_json::to_string(value).unwrap_or_else(|e| {
        tracing::warn!("Failed to serialize tool call arguments: {}", e);
        "{}".to_string()
    });
    serializer.serialize_str(&s)
}

/// Deserialize arguments from either a JSON string or an inline object.
/// The API returns a string, but we also accept an object for flexibility.
fn deserialize_args_from_string_or_object<'de, D>(
    deserializer: D,
) -> Result<serde_json::Value, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    match raw {
        serde_json::Value::String(s) => serde_json::from_str(&s).map_err(serde::de::Error::custom),
        other => Ok(other),
    }
}

/// Token usage information from an LLM response
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Usage {
    /// Number of tokens in the prompt
    #[serde(default, rename = "prompt_tokens")]
    pub input_tokens: usize,
    /// Number of tokens in the completion
    #[serde(default, rename = "completion_tokens")]
    pub output_tokens: usize,
    /// Total tokens used
    #[serde(default, rename = "total_tokens")]
    pub total_tokens: usize,
}

/// Chat completion response
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    /// Response content (None if tool calls)
    pub content: Option<String>,

    /// Tool calls
    pub tool_calls: Vec<ToolCall>,

    /// Reasoning content (for models that support it)
    pub reasoning_content: Option<String>,

    /// Token usage information (if provided by API)
    #[serde(default)]
    pub usage: Option<Usage>,
}

impl ChatResponse {
    /// Whether the response contains tool calls
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
    /// Create a text response
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            reasoning_content: None,
            usage: None,
        }
    }

    /// Create a tool call response
    pub fn tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content: None,
            tool_calls,
            reasoning_content: None,
            usage: None,
        }
    }

    /// Get token usage, converting from TokenUsage if needed
    pub fn token_usage(&self) -> Option<crate::token_tracker::TokenUsage> {
        self.usage.as_ref().map(|u| {
            crate::token_tracker::TokenUsage::from_api_fields(u.input_tokens, u.output_tokens)
        })
    }
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// Reason why the stream finished
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    /// Model finished generating normally
    Stop,
    /// Model wants to call tool(s)
    ToolCalls,
    /// Hit max token limit
    Length,
    /// Other/unknown reason
    Other(String),
}

impl FinishReason {
    /// Parse from the string value returned by OpenAI-compatible APIs.
    pub fn from_api_str(s: &str) -> Self {
        match s {
            "stop" => Self::Stop,
            "tool_calls" => Self::ToolCalls,
            "length" => Self::Length,
            other => Self::Other(other.to_string()),
        }
    }
}

/// A single incremental chunk from a streaming response.
#[derive(Debug, Clone)]
pub struct ChatStreamChunk {
    /// The incremental content in this chunk
    pub delta: ChatStreamDelta,
    /// Set on the final chunk to indicate why the stream ended
    pub finish_reason: Option<FinishReason>,
    /// Token usage (may be present in the final chunk)
    pub usage: Option<Usage>,
}

impl ChatStreamChunk {
    /// Create a ChatStreamChunk that wraps an entire non-streaming response.
    ///
    /// Used by the default `chat_stream` implementation.
    pub fn from_response(response: ChatResponse) -> Self {
        let finish_reason = if response.has_tool_calls() {
            Some(FinishReason::ToolCalls)
        } else {
            Some(FinishReason::Stop)
        };
        Self {
            delta: ChatStreamDelta {
                content: response.content,
                reasoning_content: response.reasoning_content,
                tool_calls: response
                    .tool_calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, tc)| ToolCallDelta {
                        index: i,
                        id: Some(tc.id),
                        function_name: Some(tc.function.name),
                        function_arguments: Some(
                            serde_json::to_string(&tc.function.arguments).unwrap_or_default(),
                        ),
                    })
                    .collect(),
            },
            finish_reason,
            usage: response.usage,
        }
    }
}

/// Incremental delta content within a stream chunk.
#[derive(Debug, Clone, Default)]
pub struct ChatStreamDelta {
    /// Incremental text content
    pub content: Option<String>,
    /// Incremental reasoning/thinking content
    pub reasoning_content: Option<String>,
    /// Tool call deltas (may arrive across multiple chunks)
    pub tool_calls: Vec<ToolCallDelta>,
}

/// Incremental tool call data within a stream chunk.
#[derive(Debug, Clone)]
pub struct ToolCallDelta {
    /// Index of this tool call (for matching across chunks)
    pub index: usize,
    /// Tool call ID (only present in the first chunk for this tool call)
    pub id: Option<String>,
    /// Function name (only present in the first chunk for this tool call)
    pub function_name: Option<String>,
    /// Incremental function arguments string
    pub function_arguments: Option<String>,
}
