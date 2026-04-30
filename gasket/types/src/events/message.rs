use super::channel::ChannelType;
use super::session::SessionKey;
use super::stream::WebSocketMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

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

    /// User-explicit phase override (e.g. "planning", "execute").
    /// Set by CLI commands like `/plan <content>` or API field `override_phase`.
    /// When present, the session layer writes this directly to persisted phase state,
    /// bypassing any LLM-driven phase inference.
    #[serde(default)]
    pub override_phase: Option<String>,
}

impl InboundMessage {
    /// Get the session key for this message
    pub fn session_key(&self) -> SessionKey {
        SessionKey::new(self.channel.clone(), &self.chat_id)
    }
}

// ── Target ──────────────────────────────────────────────────

/// Delivery target for an outbound message.
///
/// Replaces the magic string `"*"` for broadcasts with a proper enum.
/// Custom serde preserves the `"chat_id": "user-123"` JSON format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// Deliver to a specific chat
    Chat(String),
    /// Broadcast to all connections
    Broadcast,
}

impl Serialize for Target {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Target::Chat(id) => s.serialize_str(id),
            Target::Broadcast => s.serialize_str("*"),
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "*" {
            Ok(Target::Broadcast)
        } else {
            Ok(Target::Chat(s))
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Chat(id) => write!(f, "{}", id),
            Target::Broadcast => write!(f, "*"),
        }
    }
}

// ── Outbound Payload ───────────────────────────────────────

/// Outbound message payload — either plain text or a structured stream event.
///
/// Makes the mutual exclusivity explicit at the type level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OutboundPayload {
    /// Plain text content (for non-streaming channels)
    Text(String),
    /// Structured stream event (for streaming channels like WebSocket)
    Stream(WebSocketMessage),
}

// ── Outbound Message ───────────────────────────────────────

/// Outbound message to a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboundMessage {
    /// Target channel
    pub channel: ChannelType,

    /// Delivery target (specific chat or broadcast).
    /// Serialized as `"chat_id"` in JSON for backward compatibility.
    #[serde(rename = "chat_id")]
    pub target: Target,

    /// Message payload (text or structured stream event)
    pub payload: OutboundPayload,

    /// Additional metadata
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,

    /// Trail trace ID for end-to-end request tracking.
    #[serde(default)]
    pub trace_id: Option<String>,
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
            target: Target::Chat(chat_id.into()),
            payload: OutboundPayload::Text(content.into()),
            metadata: None,
            trace_id: None,
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
            target: Target::Chat(chat_id.into()),
            payload: OutboundPayload::Stream(ws_message),
            metadata: None,
            trace_id: None,
        }
    }

    /// Get the chat ID as a string slice
    pub fn chat_id(&self) -> &str {
        match &self.target {
            Target::Chat(id) => id,
            Target::Broadcast => "*",
        }
    }

    /// Get the text content (empty string if this is a stream payload)
    pub fn content(&self) -> &str {
        match &self.payload {
            OutboundPayload::Text(s) => s,
            OutboundPayload::Stream(_) => "",
        }
    }

    /// Get the stream message, if this is a stream payload
    pub fn ws_message(&self) -> Option<&WebSocketMessage> {
        match &self.payload {
            OutboundPayload::Stream(msg) => Some(msg),
            OutboundPayload::Text(_) => None,
        }
    }

    /// Returns true if this message is a broadcast
    pub fn is_broadcast(&self) -> bool {
        matches!(self.target, Target::Broadcast)
    }

    /// Create a broadcast outbound message with plain text content.
    pub fn broadcast(channel: ChannelType, content: impl Into<String>) -> Self {
        Self {
            channel,
            target: Target::Broadcast,
            payload: OutboundPayload::Text(content.into()),
            metadata: None,
            trace_id: None,
        }
    }

    /// Create a broadcast outbound message with a structured WebSocket message.
    pub fn broadcast_ws_message(channel: ChannelType, ws_message: WebSocketMessage) -> Self {
        Self {
            channel,
            target: Target::Broadcast,
            payload: OutboundPayload::Stream(ws_message),
            metadata: None,
            trace_id: None,
        }
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
