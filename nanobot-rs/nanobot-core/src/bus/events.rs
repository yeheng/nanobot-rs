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

    /// Message content
    pub content: String,

    /// Additional metadata
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,

    /// Trail trace ID for end-to-end request tracking.
    #[serde(default)]
    pub trace_id: Option<String>,
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
