//! Stream buffering utilities for WebSocket message ordering
//!
//! Simplified version: no merging, just preserve message order.
//! The frontend is responsible for handling different message types.

use crate::bus::events::WebSocketMessage;

/// Buffered events for a single subagent or agent execution.
///
/// Simplified: just stores messages, no merging logic.
/// The frontend handles message type differentiation.
#[derive(Debug, Default)]
pub struct BufferedEvents {
    /// Collected WebSocket messages
    pub messages: Vec<WebSocketMessage>,
    /// Whether the execution has completed
    pub completed: bool,
}

impl BufferedEvents {
    /// Create a new empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a message to the buffer.
    pub fn push(&mut self, message: WebSocketMessage) {
        self.messages.push(message);
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the number of buffered messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Flush messages in original order (no merging).
    ///
    /// The frontend is responsible for handling different message types.
    /// This method just returns messages in the order they were received.
    pub fn flush(&mut self) -> Vec<WebSocketMessage> {
        std::mem::take(&mut self.messages)
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.completed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_buffer() {
        let mut buffer = BufferedEvents::new();
        let flushed = buffer.flush();
        assert!(flushed.is_empty());
    }

    #[test]
    fn test_order_preserved() {
        let mut buffer = BufferedEvents::new();

        // Add messages in specific order
        buffer.push(WebSocketMessage::thinking("thinking1"));
        buffer.push(WebSocketMessage::tool_start("tool1", None));
        buffer.push(WebSocketMessage::content("content1"));
        buffer.push(WebSocketMessage::tool_end("tool1", Some("result".to_string())));
        buffer.push(WebSocketMessage::done());

        let flushed = buffer.flush();

        // Verify order is preserved (no merging)
        assert_eq!(flushed.len(), 5);

        // Order should match insertion order
        assert!(matches!(flushed[0], WebSocketMessage::Thinking { .. }));
        assert!(matches!(flushed[1], WebSocketMessage::ToolStart { .. }));
        assert!(matches!(flushed[2], WebSocketMessage::Content { .. }));
        assert!(matches!(flushed[3], WebSocketMessage::ToolEnd { .. }));
        assert!(matches!(flushed[4], WebSocketMessage::Done));
    }

    #[test]
    fn test_completed_flag() {
        let mut buffer = BufferedEvents::new();
        assert!(!buffer.completed);
        buffer.completed = true;
        assert!(buffer.completed);
    }

    #[test]
    fn test_clear() {
        let mut buffer = BufferedEvents::new();
        buffer.push(WebSocketMessage::content("test"));
        buffer.completed = true;

        buffer.clear();

        assert!(buffer.is_empty());
        assert!(!buffer.completed);
    }
}
