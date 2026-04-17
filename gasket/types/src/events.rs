//! Message events and channel types.
//!
//! This module defines the core data types for message passing between
//! different channels in the gasket system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Channel type identifier.
///
/// Uses an enum for known channels with a Custom variant for extensibility.
/// This provides compile-time exhaustiveness checking while still allowing
/// new channels to be added without modifying core code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum ChannelType {
    /// Telegram channel
    Telegram,
    /// Discord channel
    Discord,
    /// Slack channel
    Slack,
    /// DingTalk (钉钉) channel
    Dingtalk,
    /// Feishu (飞书) channel
    Feishu,
    /// WeCom (企业微信) channel
    Wecom,
    /// WebSocket channel
    WebSocket,
    /// CLI (command-line interface) channel
    #[default]
    Cli,
    /// Custom channel for extensibility
    Custom(String),
}

// Custom serialization to maintain backward compatibility with string format
impl Serialize for ChannelType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChannelType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

impl ChannelType {
    /// Get the channel name as a string slice
    pub fn as_str(&self) -> &str {
        match self {
            ChannelType::Telegram => "telegram",
            ChannelType::Discord => "discord",
            ChannelType::Slack => "slack",
            ChannelType::Dingtalk => "dingtalk",
            ChannelType::Feishu => "feishu",
            ChannelType::Wecom => "wecom",
            ChannelType::WebSocket => "websocket",
            ChannelType::Cli => "cli",
            ChannelType::Custom(name) => name,
        }
    }

    /// Create a channel type from a string
    pub fn new(name: impl Into<String>) -> Self {
        let s = name.into().to_lowercase();
        match s.as_str() {
            "telegram" => ChannelType::Telegram,
            "discord" => ChannelType::Discord,
            "slack" => ChannelType::Slack,
            "dingtalk" => ChannelType::Dingtalk,
            "feishu" => ChannelType::Feishu,
            "wecom" => ChannelType::Wecom,
            "websocket" => ChannelType::WebSocket,
            "cli" => ChannelType::Cli,
            _ => ChannelType::Custom(s),
        }
    }

    /// Check if this channel supports real-time streaming.
    ///
    /// Streaming channels receive incremental LLM output (thinking, content, tool events)
    /// and forward them to the client in real-time. Non-streaming channels only receive
    /// the final aggregated response.
    pub fn supports_streaming(&self) -> bool {
        matches!(self, ChannelType::WebSocket)
    }
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for ChannelType {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ChannelType {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

// ── SessionKey ───────────────────────────────────────────────

/// Strongly-typed session identifier.
///
/// Replaces stringly-typed `session_key: &str` parameters with a structured
/// type that preserves the channel and chat_id components, eliminating
/// unnecessary heap allocations from `format!("{}:{}", channel, chat_id)`.
///
/// # Example
///
/// ```
/// use gasket_types::{SessionKey, ChannelType};
///
/// let key = SessionKey::new(ChannelType::Telegram, "chat-123");
/// assert_eq!(key.channel, ChannelType::Telegram);
/// assert_eq!(key.chat_id, "chat-123");
/// assert_eq!(key.to_string(), "telegram:chat-123");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// The channel type for this session.
    pub channel: ChannelType,
    /// The chat/user identifier within the channel.
    pub chat_id: String,
}

impl SessionKey {
    /// Create a new session key from a channel and chat ID.
    pub fn new(channel: ChannelType, chat_id: impl Into<String>) -> Self {
        Self {
            channel,
            chat_id: chat_id.into(),
        }
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.channel, self.chat_id)
    }
}

impl SessionKey {
    /// Parse a session key from a string.
    ///
    /// Returns `None` if the format is invalid (missing ':' separator).
    ///
    /// # Example
    ///
    /// ```
    /// use gasket_types::SessionKey;
    ///
    /// let key = SessionKey::parse("telegram:chat-123");
    /// assert!(key.is_some());
    ///
    /// let invalid = SessionKey::parse("invalid_format");
    /// assert!(invalid.is_none());
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        match parts.as_slice() {
            [channel, chat_id] => Some(Self::new(ChannelType::new(*channel), *chat_id)),
            _ => None,
        }
    }

    /// Parse a session key from a string, returning an error on failure.
    ///
    /// # Example
    ///
    /// ```
    /// use gasket_types::SessionKey;
    ///
    /// let key = SessionKey::try_parse("telegram:chat-123").unwrap();
    /// assert_eq!(key.chat_id, "chat-123");
    ///
    /// let result = SessionKey::try_parse("invalid");
    /// assert!(result.is_err());
    /// ```
    pub fn try_parse(s: impl AsRef<str>) -> Result<Self, SessionKeyParseError> {
        Self::parse(s.as_ref())
            .ok_or_else(|| SessionKeyParseError::InvalidFormat(s.as_ref().to_string()))
    }
}

impl From<&str> for SessionKey {
    /// Parse a session key from a string.
    ///
    /// # Panics
    ///
    /// Panics if the format is invalid (missing ':' separator).
    /// Use [`SessionKey::parse`] or [`SessionKey::try_parse`] for fallible versions.
    fn from(s: &str) -> Self {
        Self::parse(s).unwrap_or_else(|| panic!("Invalid session key format: {}", s))
    }
}

impl From<String> for SessionKey {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

/// Error type for session key parsing failures.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Failed to parse session key: {0}")]
pub enum SessionKeyParseError {
    #[error("Invalid format (expected 'channel:chat_id'): {0}")]
    InvalidFormat(String),
}

// ── InboundMessage ───────────────────────────────────────────

/// Inbound message from a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboundMessage {
    /// Source channel
    pub channel: ChannelType,

    /// Sender ID
    pub sender_id: String,

    /// Chat ID (for routing responses)
    pub chat_id: String,

    /// Message content
    pub content: String,

    /// Media attachments (if any)
    #[serde(default)]
    pub media: Option<Vec<MediaAttachment>>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,

    /// Timestamp
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,

    /// Trail trace ID for end-to-end request tracking.
    #[serde(default)]
    pub trace_id: Option<String>,
}

impl InboundMessage {
    /// Get the session key for this message
    pub fn session_key(&self) -> SessionKey {
        SessionKey::new(self.channel.clone(), &self.chat_id)
    }
}

/// Outbound message to a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboundMessage {
    /// Target channel
    pub channel: ChannelType,

    /// Target chat ID
    pub chat_id: String,

    /// Message content (plain text)
    pub content: String,

    /// Additional metadata
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,

    /// Trail trace ID for end-to-end request tracking.
    #[serde(default)]
    pub trace_id: Option<String>,

    /// Structured WebSocket message (for real-time streaming)
    /// When set, this takes precedence over `content` for WebSocket channels
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ws_message: Option<WebSocketMessage>,
}

impl OutboundMessage {
    /// Create a new outbound message with plain text content
    pub fn new(
        channel: ChannelType,
        chat_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel,
            chat_id: chat_id.into(),
            content: content.into(),
            metadata: None,
            trace_id: None,
            ws_message: None,
        }
    }

    /// Create an outbound message with a structured WebSocket message
    pub fn with_ws_message(
        channel: ChannelType,
        chat_id: impl Into<String>,
        ws_message: WebSocketMessage,
    ) -> Self {
        Self {
            channel,
            chat_id: chat_id.into(),
            content: String::new(),
            metadata: None,
            trace_id: None,
            ws_message: Some(ws_message),
        }
    }

    /// Check if this message has a structured WebSocket payload
    pub fn has_ws_message(&self) -> bool {
        self.ws_message.is_some()
    }

    /// Returns true if this message is a broadcast (chat_id == "*").
    pub fn is_broadcast(&self) -> bool {
        self.chat_id == "*"
    }

    /// Create a broadcast outbound message with plain text content.
    pub fn broadcast(channel: ChannelType, content: impl Into<String>) -> Self {
        Self::new(channel, "*", content)
    }

    /// Create a broadcast outbound message with a structured WebSocket message.
    pub fn broadcast_ws_message(channel: ChannelType, ws_message: WebSocketMessage) -> Self {
        Self::with_ws_message(channel, "*", ws_message)
    }
}

/// Media attachment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaAttachment {
    /// Media type (image, audio, video, etc.)
    pub media_type: String,

    /// URL or base64 data
    pub data: String,

    /// Optional caption
    #[serde(default)]
    pub caption: Option<String>,
}

// ── Unified Stream Event ────────────────────────────────────

/// Unified stream event for real-time streaming across the entire pipeline.
///
/// This single event type eliminates the "中间商赚差价" pattern of converting
/// between `StreamEvent` -> `SubagentEvent` -> `WebSocketMessage`.
///
/// The `agent_id` field distinguishes between:
/// - `None`: Main agent events
/// - `Some(id)`: Subagent events (id is the subagent's UUID)
///
/// # Protocol (JSON representation for WebSocket)
///
/// ```json
/// // Main agent thinking/reasoning
/// {"type": "thinking", "agent_id": null, "content": "..."}
///
/// // Subagent thinking (agent_id identifies the subagent)
/// {"type": "thinking", "agent_id": "uuid-123", "content": "..."}
///
/// // Tool call started
/// {"type": "tool_start", "agent_id": null, "name": "tool_name", "arguments": "{...}"}
///
/// // Tool call completed
/// {"type": "tool_end", "agent_id": null, "name": "tool_name", "output": "..."}
///
/// // Streaming content chunk
/// {"type": "content", "agent_id": null, "content": "..."}
///
/// // Stream completed
/// {"type": "done", "agent_id": null}
///
/// // Subagent lifecycle events
/// {"type": "subagent_started", "agent_id": "uuid-123", "task": "...", "index": 1}
/// {"type": "subagent_completed", "agent_id": "uuid-123", "index": 1, "summary": "...", "tool_count": 5}
/// {"type": "subagent_error", "agent_id": "uuid-123", "index": 1, "error": "..."}
///
/// // Token statistics (main agent only, typically)
/// {"type": "token_stats", "agent_id": null, "input_tokens": 1000, "output_tokens": 500, ...}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Thinking/reasoning content from the LLM
    Thinking {
        /// Agent ID (`None` for main agent, `Some(uuid)` for subagent)
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        content: Arc<str>,
    },

    /// A tool call has started
    ToolStart {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        name: Arc<str>,
        #[serde(default)]
        arguments: Option<Arc<str>>,
    },

    /// A tool call has completed
    ToolEnd {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        name: Arc<str>,
        #[serde(default)]
        output: Option<Arc<str>>,
    },

    /// Streaming content chunk
    Content {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        content: Arc<str>,
    },

    /// Stream has completed for this iteration
    Done {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
    },

    /// Token usage statistics
    TokenStats {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        input_tokens: usize,
        output_tokens: usize,
        total_tokens: usize,
        cost: f64,
        currency: Arc<str>,
    },

    // === Subagent Lifecycle Events ===
    /// Subagent started execution
    SubagentStarted {
        agent_id: Arc<str>,
        task: Arc<str>,
        index: u32,
    },

    /// Subagent completed execution
    SubagentCompleted {
        agent_id: Arc<str>,
        index: u32,
        summary: Arc<str>,
        tool_count: u32,
    },

    /// Subagent encountered an error
    SubagentError {
        agent_id: Arc<str>,
        index: u32,
        error: Arc<str>,
    },

    /// Plain text message (legacy support for non-streaming channels)
    Text {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        content: Arc<str>,
    },
}

/// Clean user-facing event for WebSocket and outbound channels.
///
/// This is the data-plane event type — it contains only what the end-user
/// should see. Control-plane events (TokenStats, Subagent lifecycle) are
/// intentionally excluded and handled internally via `SystemEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Thinking/reasoning content from the LLM
    Thinking { content: Arc<str> },

    /// A tool call has started
    ToolStart {
        name: Arc<str>,
        #[serde(default)]
        arguments: Option<Arc<str>>,
    },

    /// A tool call has completed
    ToolEnd {
        name: Arc<str>,
        #[serde(default)]
        output: Option<Arc<str>>,
    },

    /// Streaming content chunk
    Content { content: Arc<str> },

    /// Stream has completed for this iteration
    Done,

    /// Plain text message (legacy support for non-streaming channels)
    Text { content: Arc<str> },

    /// Error message
    Error { message: Arc<str> },
}

impl ChatEvent {
    /// Create a content message
    pub fn content(content: impl Into<String>) -> Self {
        Self::Content {
            content: Arc::from(content.into()),
        }
    }

    /// Create a thinking message
    pub fn thinking(content: impl Into<String>) -> Self {
        Self::Thinking {
            content: Arc::from(content.into()),
        }
    }

    /// Create a tool_start message
    pub fn tool_start(name: impl Into<String>, arguments: Option<String>) -> Self {
        Self::ToolStart {
            name: Arc::from(name.into()),
            arguments: arguments.map(Arc::from),
        }
    }

    /// Create a tool_end message
    pub fn tool_end(name: impl Into<String>, output: Option<String>) -> Self {
        Self::ToolEnd {
            name: Arc::from(name.into()),
            output: output.map(Arc::from),
        }
    }

    /// Create a done message
    pub fn done() -> Self {
        Self::Done
    }

    /// Create a text message
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            content: Arc::from(content.into()),
        }
    }

    /// Create an error message
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: Arc::from(message.into()),
        }
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"type":"text","content":"serialization error"}"#.to_string())
    }
}

/// WebSocket message type — a clean alias for `ChatEvent`.
pub type WebSocketMessage = ChatEvent;

impl StreamEvent {
    // === Main Agent Event Constructors ===

    /// Create a thinking message for the main agent
    pub fn thinking(content: impl Into<String>) -> Self {
        Self::Thinking {
            agent_id: None,
            content: Arc::from(content.into()),
        }
    }

    /// Create a tool_start message for the main agent
    pub fn tool_start(name: impl Into<String>, arguments: Option<String>) -> Self {
        Self::ToolStart {
            agent_id: None,
            name: Arc::from(name.into()),
            arguments: arguments.map(Arc::from),
        }
    }

    /// Create a tool_end message for the main agent
    pub fn tool_end(name: impl Into<String>, output: Option<String>) -> Self {
        Self::ToolEnd {
            agent_id: None,
            name: Arc::from(name.into()),
            output: output.map(Arc::from),
        }
    }

    /// Create a content message for the main agent
    pub fn content(content: impl Into<String>) -> Self {
        Self::Content {
            agent_id: None,
            content: Arc::from(content.into()),
        }
    }

    /// Create a done message for the main agent
    pub fn done() -> Self {
        Self::Done { agent_id: None }
    }

    /// Create a token_stats message for the main agent
    pub fn token_stats(
        input_tokens: usize,
        output_tokens: usize,
        total_tokens: usize,
        cost: f64,
        currency: impl Into<String>,
    ) -> Self {
        Self::TokenStats {
            agent_id: None,
            input_tokens,
            output_tokens,
            total_tokens,
            cost,
            currency: Arc::from(currency.into()),
        }
    }

    /// Create a plain text message (legacy)
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            agent_id: None,
            content: Arc::from(content.into()),
        }
    }

    // === Subagent Event Constructors ===

    /// Create a thinking message for a subagent
    pub fn subagent_thinking(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Thinking {
            agent_id: Some(Arc::from(id.into())),
            content: Arc::from(content.into()),
        }
    }

    /// Create a tool_start message for a subagent
    pub fn subagent_tool_start(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Option<String>,
    ) -> Self {
        Self::ToolStart {
            agent_id: Some(Arc::from(id.into())),
            name: Arc::from(name.into()),
            arguments: arguments.map(Arc::from),
        }
    }

    /// Create a tool_end message for a subagent
    pub fn subagent_tool_end(
        id: impl Into<String>,
        name: impl Into<String>,
        output: Option<String>,
    ) -> Self {
        Self::ToolEnd {
            agent_id: Some(Arc::from(id.into())),
            name: Arc::from(name.into()),
            output: output.map(Arc::from),
        }
    }

    /// Create a content message for a subagent
    pub fn subagent_content(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Content {
            agent_id: Some(Arc::from(id.into())),
            content: Arc::from(content.into()),
        }
    }

    /// Create a subagent_started message
    pub fn subagent_started(id: impl Into<String>, task: impl Into<String>, index: u32) -> Self {
        Self::SubagentStarted {
            agent_id: Arc::from(id.into()),
            task: Arc::from(task.into()),
            index,
        }
    }

    /// Create a subagent_completed message
    pub fn subagent_completed(
        id: impl Into<String>,
        index: u32,
        summary: impl Into<String>,
        tool_count: u32,
    ) -> Self {
        Self::SubagentCompleted {
            agent_id: Arc::from(id.into()),
            index,
            summary: Arc::from(summary.into()),
            tool_count,
        }
    }

    /// Create a subagent_error message
    pub fn subagent_error(id: impl Into<String>, index: u32, error: impl Into<String>) -> Self {
        Self::SubagentError {
            agent_id: Arc::from(id.into()),
            index,
            error: Arc::from(error.into()),
        }
    }

    // === Utility Methods ===

    /// Get the agent_id if this is a subagent event
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Thinking { agent_id, .. } => agent_id.as_deref(),
            Self::ToolStart { agent_id, .. } => agent_id.as_deref(),
            Self::ToolEnd { agent_id, .. } => agent_id.as_deref(),
            Self::Content { agent_id, .. } => agent_id.as_deref(),
            Self::Done { agent_id } => agent_id.as_deref(),
            Self::TokenStats { agent_id, .. } => agent_id.as_deref(),
            Self::Text { agent_id, .. } => agent_id.as_deref(),
            // Subagent lifecycle events always have an agent_id
            Self::SubagentStarted { agent_id, .. } => Some(agent_id),
            Self::SubagentCompleted { agent_id, .. } => Some(agent_id),
            Self::SubagentError { agent_id, .. } => Some(agent_id),
        }
    }

    /// Check if this event is from a subagent
    pub fn is_subagent_event(&self) -> bool {
        self.agent_id().is_some()
    }

    /// Check if this is a main agent event
    pub fn is_main_agent_event(&self) -> bool {
        self.agent_id().is_none()
    }

    /// Convert to a user-facing `ChatEvent` if this is a main-agent data event.
    ///
    /// Returns `None` for system events (`TokenStats`, subagent lifecycle)
    /// and for subagent events (anything with `agent_id` set).
    pub fn to_chat_event(&self) -> Option<ChatEvent> {
        if self.is_subagent_event() {
            return None;
        }
        match self {
            Self::Thinking { content, .. } => Some(ChatEvent::Thinking {
                content: Arc::clone(content),
            }),
            Self::ToolStart {
                name, arguments, ..
            } => Some(ChatEvent::ToolStart {
                name: Arc::clone(name),
                arguments: arguments.as_ref().map(Arc::clone),
            }),
            Self::ToolEnd { name, output, .. } => Some(ChatEvent::ToolEnd {
                name: Arc::clone(name),
                output: output.as_ref().map(Arc::clone),
            }),
            Self::Content { content, .. } => Some(ChatEvent::Content {
                content: Arc::clone(content),
            }),
            Self::Done { .. } => Some(ChatEvent::Done),
            Self::Text { content, .. } => Some(ChatEvent::Text {
                content: Arc::clone(content),
            }),
            Self::TokenStats { .. }
            | Self::SubagentStarted { .. }
            | Self::SubagentCompleted { .. }
            | Self::SubagentError { .. } => None,
        }
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"type":"text","content":"serialization error"}"#.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_type_constructors() {
        assert_eq!(ChannelType::Telegram.as_str(), "telegram");
        assert_eq!(ChannelType::Discord.as_str(), "discord");
        assert_eq!(ChannelType::Slack.as_str(), "slack");
        assert_eq!(ChannelType::Dingtalk.as_str(), "dingtalk");
        assert_eq!(ChannelType::Feishu.as_str(), "feishu");
        assert_eq!(ChannelType::Wecom.as_str(), "wecom");
        assert_eq!(ChannelType::Cli.as_str(), "cli");
    }

    #[test]
    fn test_channel_type_from_str() {
        let channel = ChannelType::from("custom_channel");
        assert_eq!(channel.as_str(), "custom_channel");
    }

    #[test]
    fn test_channel_type_normalization() {
        let channel = ChannelType::new("TELEGRAM");
        assert_eq!(channel.as_str(), "telegram");
        assert!(matches!(channel, ChannelType::Telegram));
    }

    #[test]
    fn test_channel_type_serialization() {
        let channel = ChannelType::Telegram;
        let json = serde_json::to_string(&channel).unwrap();
        assert_eq!(json, "\"telegram\"");

        let deserialized: ChannelType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ChannelType::Telegram);
    }

    #[test]
    fn test_session_key_parse_valid() {
        let key = SessionKey::parse("telegram:chat-123").unwrap();
        assert_eq!(key.channel, ChannelType::Telegram);
        assert_eq!(key.chat_id, "chat-123");
    }

    #[test]
    fn test_session_key_parse_invalid() {
        assert!(SessionKey::parse("invalid_format").is_none());
        assert!(SessionKey::parse("").is_none());
    }

    #[test]
    fn test_session_key_roundtrip() {
        let original = SessionKey::new(ChannelType::WebSocket, "session-abc");
        let string = original.to_string();
        let parsed = SessionKey::parse(&string).unwrap();
        assert_eq!(original, parsed);
    }

    // === Unified StreamEvent tests ===

    #[test]
    fn test_main_agent_events() {
        // Test that main agent events have no agent_id
        let thinking = StreamEvent::thinking("test");
        assert!(thinking.agent_id().is_none());
        assert!(thinking.is_main_agent_event());

        let content = StreamEvent::content("hello");
        assert!(content.agent_id().is_none());
        assert!(!content.is_subagent_event());

        let done = StreamEvent::done();
        assert!(done.agent_id().is_none());
    }

    #[test]
    fn test_subagent_events() {
        // Test that subagent events have agent_id
        let thinking = StreamEvent::subagent_thinking("uuid-123", "test");
        assert_eq!(thinking.agent_id(), Some("uuid-123"));
        assert!(thinking.is_subagent_event());

        let content = StreamEvent::subagent_content("uuid-123", "hello");
        assert_eq!(content.agent_id(), Some("uuid-123"));
    }

    #[test]
    fn test_subagent_started_serialization() {
        let msg = StreamEvent::subagent_started("id-123", "Search docs", 1);
        let json = msg.to_json();
        assert!(json.contains(r#""type":"subagent_started"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""task":"Search docs"#));
        assert!(json.contains(r#""index":1"#));
    }

    #[test]
    fn test_subagent_thinking_serialization() {
        let msg = StreamEvent::subagent_thinking("id-123", "Analyzing...");
        let json = msg.to_json();
        assert!(json.contains(r#""type":"thinking"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""content":"Analyzing..."#));
    }

    #[test]
    fn test_subagent_completed_serialization() {
        let msg = StreamEvent::subagent_completed("id-123", 1, "Done", 5);
        let json = msg.to_json();
        assert!(json.contains(r#""type":"subagent_completed"#));
        assert!(json.contains(r#""tool_count":5"#));
    }

    #[test]
    fn test_subagent_message_deserialization() {
        let json =
            r#"{"type":"subagent_started","agent_id":"id-123","task":"Test task","index":1}"#;
        let msg: StreamEvent = serde_json::from_str(json).unwrap();
        match msg {
            StreamEvent::SubagentStarted {
                agent_id,
                task,
                index,
            } => {
                assert_eq!(agent_id.as_ref(), "id-123");
                assert_eq!(task.as_ref(), "Test task");
                assert_eq!(index, 1);
            }
            _ => panic!("Expected SubagentStarted"),
        }
    }

    #[test]
    fn test_subagent_tool_messages() {
        let start_msg = StreamEvent::subagent_tool_start(
            "id-123",
            "read_file",
            Some(r#"{"path":"/test.txt"}"#.to_string()),
        );
        let json = start_msg.to_json();
        assert!(json.contains(r#""type":"tool_start"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""name":"read_file"#));

        let end_msg = StreamEvent::subagent_tool_end(
            "id-123",
            "read_file",
            Some("file contents".to_string()),
        );
        let json = end_msg.to_json();
        assert!(json.contains(r#""type":"tool_end"#));
        assert!(json.contains(r#""output":"file contents"#));
    }

    #[test]
    fn test_token_stats_event() {
        let stats = StreamEvent::token_stats(1000, 500, 1500, 0.01, "USD");
        match stats {
            StreamEvent::TokenStats {
                agent_id,
                input_tokens,
                output_tokens,
                total_tokens,
                cost,
                currency,
            } => {
                assert!(agent_id.is_none());
                assert_eq!(input_tokens, 1000);
                assert_eq!(output_tokens, 500);
                assert_eq!(total_tokens, 1500);
                assert!((cost - 0.01).abs() < 0.0001);
                assert_eq!(currency.as_ref(), "USD");
            }
            _ => panic!("Expected TokenStats"),
        }
    }

    #[test]
    fn test_websocket_message_is_chat_event() {
        // WebSocketMessage is now an alias for ChatEvent
        let msg: WebSocketMessage = ChatEvent::thinking("test");
        assert!(matches!(msg, ChatEvent::Thinking { .. }));
    }
}
