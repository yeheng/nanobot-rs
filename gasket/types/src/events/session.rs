use std::fmt;

use super::channel::ChannelType;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
