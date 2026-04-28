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
    /// DingTalk (钉钉) channel
    Dingtalk,
    /// Feishu (飞书) channel
    Feishu,
    /// WeCom (企业微信) channel
    Wecom,
    /// WeChat (个人微信) channel
    Wechat,
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
            ChannelType::Wechat => "wechat",
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
            "wechat" => ChannelType::Wechat,
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
        assert_eq!(ChannelType::Wechat.as_str(), "wechat");
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
}
