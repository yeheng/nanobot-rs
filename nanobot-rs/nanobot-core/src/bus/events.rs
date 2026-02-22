//! Message events

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Channel type identifier.
///
/// Uses a newtype pattern instead of an enum to allow extensibility.
/// New channels can be added without modifying core code.
///
/// Pre-defined constants are provided for well-known channels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelType(String);

impl ChannelType {
    /// Create a new channel type from a string
    pub fn new(name: impl Into<String>) -> Self {
        let s = name.into();
        // Normalize to lowercase for consistency
        Self(s.to_lowercase())
    }

    /// Get the channel name as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

impl Default for ChannelType {
    fn default() -> Self {
        cli()
    }
}

// Pre-defined channel type constructors
// These are the standard channels supported out of the box

/// Telegram channel
pub fn telegram() -> ChannelType {
    ChannelType::new("telegram")
}

/// Discord channel
pub fn discord() -> ChannelType {
    ChannelType::new("discord")
}

/// Slack channel
pub fn slack() -> ChannelType {
    ChannelType::new("slack")
}

/// Email channel
pub fn email() -> ChannelType {
    ChannelType::new("email")
}

/// DingTalk (钉钉) channel
pub fn dingtalk() -> ChannelType {
    ChannelType::new("dingtalk")
}

/// Feishu (飞书) channel
pub fn feishu() -> ChannelType {
    ChannelType::new("feishu")
}

/// WeCom (企业微信) channel
pub fn wecom() -> ChannelType {
    ChannelType::new("wecom")
}

/// CLI (command-line interface) channel
pub fn cli() -> ChannelType {
    ChannelType::new("cli")
}

/// Inbound message from a channel
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
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.channel, self.chat_id)
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
        assert_eq!(telegram().as_str(), "telegram");
        assert_eq!(discord().as_str(), "discord");
        assert_eq!(slack().as_str(), "slack");
        assert_eq!(email().as_str(), "email");
        assert_eq!(dingtalk().as_str(), "dingtalk");
        assert_eq!(feishu().as_str(), "feishu");
        assert_eq!(wecom().as_str(), "wecom");
        assert_eq!(cli().as_str(), "cli");
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
    }

    #[test]
    fn test_channel_type_equality() {
        assert_eq!(telegram(), ChannelType::new("telegram"));
        assert_ne!(telegram(), discord());
    }

    #[test]
    fn test_channel_type_serialization() {
        let channel = telegram();
        let json = serde_json::to_string(&channel).unwrap();
        assert_eq!(json, "\"telegram\"");

        let deserialized: ChannelType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, telegram());
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
        assert_eq!(format!("{}", telegram()), "telegram");
        assert_eq!(format!("{}", ChannelType::new("custom")), "custom");
    }
}
