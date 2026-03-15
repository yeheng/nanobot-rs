//! Message events

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

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
    /// Email channel
    Email,
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
            ChannelType::Email => "email",
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
            "email" => ChannelType::Email,
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
/// use nanobot_core::bus::events::{SessionKey, ChannelType};
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

impl From<&str> for SessionKey {
    fn from(s: &str) -> Self {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        match parts.as_slice() {
            [channel, chat_id] => Self::new(ChannelType::new(*channel), *chat_id),
            _ => panic!("Invalid session key format: {}", s),
        }
    }
}

impl From<String> for SessionKey {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

// ── InboundMessage ───────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(skip)]
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
}

/// Media attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    /// Media type (image, audio, video, etc.)
    pub media_type: String,

    /// URL or base64 data
    pub data: String,

    /// Optional caption
    #[serde(default)]
    pub caption: Option<String>,
}

// ── WebSocket Streaming Messages ─────────────────────────────

/// WebSocket streaming message types for real-time UI updates.
///
/// These message types enable the frontend to display thinking process,
/// tool calls, and content streaming in real-time.
///
/// # Protocol
///
/// ```json
/// // Thinking/reasoning content
/// {"type": "thinking", "content": "..."}
///
/// // Tool call started
/// {"type": "tool_start", "name": "tool_name", "arguments": "{...}"}
///
/// // Tool call completed
/// {"type": "tool_end", "name": "tool_name", "output": "..."}
///
/// // Streaming content chunk
/// {"type": "content", "content": "..."}
///
/// // Stream completed
/// {"type": "done"}
///
/// // Plain text message (legacy)
/// {"type": "text", "content": "..."}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebSocketMessage {
    /// Thinking/reasoning content from the LLM
    Thinking { content: String },

    /// A tool call has started
    ToolStart {
        name: String,
        #[serde(default)]
        arguments: Option<String>,
    },

    /// A tool call has completed
    ToolEnd {
        name: String,
        #[serde(default)]
        output: Option<String>,
    },

    /// Streaming content chunk
    Content { content: String },

    /// Stream has completed
    Done,

    /// Plain text message (legacy support)
    Text { content: String },
}

impl WebSocketMessage {
    /// Create a thinking message
    pub fn thinking(content: impl Into<String>) -> Self {
        Self::Thinking {
            content: content.into(),
        }
    }

    /// Create a tool_start message
    pub fn tool_start(name: impl Into<String>, arguments: Option<String>) -> Self {
        Self::ToolStart {
            name: name.into(),
            arguments,
        }
    }

    /// Create a tool_end message
    pub fn tool_end(name: impl Into<String>, output: Option<String>) -> Self {
        Self::ToolEnd {
            name: name.into(),
            output,
        }
    }

    /// Create a content message
    pub fn content(content: impl Into<String>) -> Self {
        Self::Content {
            content: content.into(),
        }
    }

    /// Create a done message
    pub fn done() -> Self {
        Self::Done
    }

    /// Create a plain text message (legacy)
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            content: content.into(),
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
        assert_eq!(ChannelType::Email.as_str(), "email");
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
    fn test_channel_type_equality() {
        assert_eq!(ChannelType::Telegram, ChannelType::new("telegram"));
        assert_ne!(ChannelType::Telegram, ChannelType::Discord);
    }

    #[test]
    fn test_channel_type_serialization() {
        let channel = ChannelType::Telegram;
        let json = serde_json::to_string(&channel).unwrap();
        // Enum variants serialize to lowercase strings for backward compatibility
        assert_eq!(json, "\"telegram\"");

        let deserialized: ChannelType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ChannelType::Telegram);
    }

    #[test]
    fn test_custom_channel_serialization() {
        let channel = ChannelType::new("wechat");
        let json = serde_json::to_string(&channel).unwrap();
        assert_eq!(json, "\"wechat\"");

        let deserialized: ChannelType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.as_str(), "wechat");
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", ChannelType::Telegram), "telegram");
        assert_eq!(format!("{}", ChannelType::new("custom")), "custom");
    }

    #[test]
    fn test_exhaustiveness() {
        // The compiler will ensure all variants are handled
        fn check_exhaustive(ct: ChannelType) -> &'static str {
            match ct {
                ChannelType::Telegram => "telegram",
                ChannelType::Discord => "discord",
                ChannelType::Slack => "slack",
                ChannelType::Email => "email",
                ChannelType::Dingtalk => "dingtalk",
                ChannelType::Feishu => "feishu",
                ChannelType::Wecom => "wecom",
                ChannelType::Cli => "cli",
                ChannelType::WebSocket => "websocket",
                ChannelType::Custom(_) => "custom",
            }
        }
        assert_eq!(check_exhaustive(ChannelType::Telegram), "telegram");
        assert_eq!(check_exhaustive(ChannelType::new("foo")), "custom");
    }
}
