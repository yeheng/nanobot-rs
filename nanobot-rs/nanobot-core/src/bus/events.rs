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

// Convenience constructor functions for backward compatibility

/// Telegram channel
pub fn telegram() -> ChannelType {
    ChannelType::Telegram
}

/// Discord channel
pub fn discord() -> ChannelType {
    ChannelType::Discord
}

/// Slack channel
pub fn slack() -> ChannelType {
    ChannelType::Slack
}

/// Email channel
pub fn email() -> ChannelType {
    ChannelType::Email
}

/// DingTalk (钉钉) channel
pub fn dingtalk() -> ChannelType {
    ChannelType::Dingtalk
}

/// Feishu (飞书) channel
pub fn feishu() -> ChannelType {
    ChannelType::Feishu
}

/// WeCom (企业微信) channel
pub fn wecom() -> ChannelType {
    ChannelType::Wecom
}

/// CLI (command-line interface) channel
pub fn cli() -> ChannelType {
    ChannelType::Cli
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
        assert!(matches!(channel, ChannelType::Telegram));
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
        // Enum variants serialize to lowercase strings for backward compatibility
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
                ChannelType::Custom(_) => "custom",
            }
        }
        assert_eq!(check_exhaustive(telegram()), "telegram");
        assert_eq!(check_exhaustive(ChannelType::new("foo")), "custom");
    }
}
