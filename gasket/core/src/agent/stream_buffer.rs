//! Stream buffering utilities for WebSocket message ordering
//!
//! Provides utilities for buffering and ordering WebSocket messages
//! to ensure a user-friendly display order:
//! 1. Thinking messages first
//! 2. Tool events (ToolStart/ToolEnd)
//! 3. Content messages last
//!
//! This ordering ensures the UI shows the thinking process before
//! the response content, avoiding interleaved display issues.

use crate::bus::events::WebSocketMessage;

/// Buffered events for a single subagent or agent execution.
///
/// Collects WebSocket messages and provides ordered flushing
/// to ensure proper display sequence.
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

    /// Flush messages in a user-friendly order:
    /// 1. All Thinking messages first
    /// 2. ToolStart/ToolEnd (if any, in original order relative to each other)
    /// 3. All Content messages last
    /// 4. Other messages (Done, Text, etc.)
    ///
    /// This ensures the UI shows the thinking process before the response content,
    /// avoiding the interleaved display issue.
    pub fn flush_ordered(&mut self) -> Vec<WebSocketMessage> {
        if self.messages.is_empty() {
            return Vec::new();
        }

        let mut thinking_msgs: Vec<WebSocketMessage> = Vec::new();
        let mut tool_msgs: Vec<WebSocketMessage> = Vec::new();
        let mut content_msgs: Vec<WebSocketMessage> = Vec::new();
        let mut other_msgs: Vec<WebSocketMessage> = Vec::new();

        for msg in self.messages.drain(..) {
            match &msg {
                WebSocketMessage::Thinking { .. } => thinking_msgs.push(msg),
                WebSocketMessage::ToolStart { .. } | WebSocketMessage::ToolEnd { .. } => {
                    tool_msgs.push(msg)
                }
                WebSocketMessage::Content { .. } => content_msgs.push(msg),
                _ => other_msgs.push(msg),
            }
        }

        // Concatenate: Thinking -> Tools -> Content -> Other
        let mut result = Vec::with_capacity(
            thinking_msgs.len() + tool_msgs.len() + content_msgs.len() + other_msgs.len(),
        );
        result.append(&mut thinking_msgs);
        result.append(&mut tool_msgs);
        result.append(&mut content_msgs);
        result.append(&mut other_msgs);

        result
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
        let flushed = buffer.flush_ordered();
        assert!(flushed.is_empty());
    }

    #[test]
    fn test_ordering() {
        let mut buffer = BufferedEvents::new();

        // Add messages in mixed order
        buffer.push(WebSocketMessage::content("content1"));
        buffer.push(WebSocketMessage::thinking("thinking1"));
        buffer.push(WebSocketMessage::tool_start("tool1", None));
        buffer.push(WebSocketMessage::content("content2"));
        buffer.push(WebSocketMessage::thinking("thinking2"));
        buffer.push(WebSocketMessage::tool_end(
            "tool1",
            Some("result".to_string()),
        ));
        buffer.push(WebSocketMessage::done());

        let flushed = buffer.flush_ordered();

        // Verify order: Thinking -> Tools -> Content -> Other
        assert_eq!(flushed.len(), 7);
        assert!(matches!(flushed[0], WebSocketMessage::Thinking { .. }));
        assert!(matches!(flushed[1], WebSocketMessage::Thinking { .. }));
        assert!(matches!(flushed[2], WebSocketMessage::ToolStart { .. }));
        assert!(matches!(flushed[3], WebSocketMessage::ToolEnd { .. }));
        assert!(matches!(flushed[4], WebSocketMessage::Content { .. }));
        assert!(matches!(flushed[5], WebSocketMessage::Content { .. }));
        assert!(matches!(flushed[6], WebSocketMessage::Done));
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
