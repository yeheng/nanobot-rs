//! Message events

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Channel type enum — enforces compile-time checks on channel names.
///
/// Using an enum instead of a bare `String` means the compiler catches typos
/// like `"teelgram"` that would silently fail at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelType {
    Telegram,
    Discord,
    Slack,
    Email,
    DingTalk,
    Feishu,
    WeCom,
    Cli,
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelType::Telegram => write!(f, "telegram"),
            ChannelType::Discord => write!(f, "discord"),
            ChannelType::Slack => write!(f, "slack"),
            ChannelType::Email => write!(f, "email"),
            ChannelType::DingTalk => write!(f, "dingtalk"),
            ChannelType::Feishu => write!(f, "feishu"),
            ChannelType::WeCom => write!(f, "wecom"),
            ChannelType::Cli => write!(f, "cli"),
        }
    }
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
