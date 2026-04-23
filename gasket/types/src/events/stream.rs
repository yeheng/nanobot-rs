use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Internal Control Signals ────────────────────────────────

/// System-internal control-plane signals that do not belong in the user-facing stream.
///
/// These events carry operational metadata (token accounting, subagent lifecycle)
/// and are handled internally rather than forwarded to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InternalSignal {
    /// Token usage statistics
    TokenStats {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_id: Option<Arc<str>>,
        input_tokens: usize,
        output_tokens: usize,
        total_tokens: usize,
        cost: f64,
        currency: Arc<str>,
    },

    /// Subagent started execution
    SubagentStarted {
        agent_id: Arc<str>,
        task: Arc<str>,
        index: u32,
    },

    /// Subagent completed execution
    SubagentCompleted {
        agent_id: Arc<str>,
        index: u32,
        summary: Arc<str>,
        tool_count: u32,
    },

    /// Subagent encountered an error
    SubagentError {
        agent_id: Arc<str>,
        index: u32,
        error: Arc<str>,
    },
}

impl InternalSignal {
    /// Create a token_stats signal for the main agent
    pub fn token_stats(
        input_tokens: usize,
        output_tokens: usize,
        total_tokens: usize,
        cost: f64,
        currency: impl Into<String>,
    ) -> Self {
        Self::TokenStats {
            agent_id: None,
            input_tokens,
            output_tokens,
            total_tokens,
            cost,
            currency: Arc::from(currency.into()),
        }
    }

    /// Create a subagent_started signal
    pub fn subagent_started(id: impl Into<String>, task: impl Into<String>, index: u32) -> Self {
        Self::SubagentStarted {
            agent_id: Arc::from(id.into()),
            task: Arc::from(task.into()),
            index,
        }
    }

    /// Create a subagent_completed signal
    pub fn subagent_completed(
        id: impl Into<String>,
        index: u32,
        summary: impl Into<String>,
        tool_count: u32,
    ) -> Self {
        Self::SubagentCompleted {
            agent_id: Arc::from(id.into()),
            index,
            summary: Arc::from(summary.into()),
            tool_count,
        }
    }

    /// Create a subagent_error signal
    pub fn subagent_error(id: impl Into<String>, index: u32, error: impl Into<String>) -> Self {
        Self::SubagentError {
            agent_id: Arc::from(id.into()),
            index,
            error: Arc::from(error.into()),
        }
    }
}

// ── Stream Event Kind ─────────────────────────────────────

/// Pure event kind without agent identity.
///
/// Extracted from `StreamEvent` to eliminate the `agent_id` repetition
/// across all six variants. Agent identity is carried by the `StreamEvent`
/// wrapper struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEventKind {
    /// Thinking/reasoning content from the LLM
    Thinking { content: Arc<str> },

    /// A tool call has started
    ToolStart {
        name: Arc<str>,
        #[serde(default)]
        arguments: Option<Arc<str>>,
    },

    /// A tool call has completed
    ToolEnd {
        name: Arc<str>,
        #[serde(default)]
        output: Option<Arc<str>>,
    },

    /// Streaming content chunk
    Content { content: Arc<str> },

    /// Stream has completed for this iteration
    Done,

    /// Plain text message (legacy support for non-streaming channels)
    Text { content: Arc<str> },
}

// ── Unified Stream Event ────────────────────────────────────

/// Unified stream event for real-time streaming across the entire pipeline.
///
/// Wraps [`StreamEventKind`] with an optional `agent_id` to distinguish
/// between main agent events (`None`) and subagent events (`Some(uuid)`).
///
/// Uses `#[serde(flatten)]` to produce the same JSON wire format as the
/// original flat enum.
///
/// # Protocol (JSON representation for WebSocket)
///
/// ```json
/// // Main agent thinking
/// {"type": "thinking", "content": "..."}
///
/// // Subagent thinking (agent_id identifies the subagent)
/// {"type": "thinking", "agent_id": "uuid-123", "content": "..."}
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
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamEvent {
    /// Agent ID (None for main agent, Some(uuid) for subagent)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<Arc<str>>,
    /// The event kind (flattened into JSON output)
    #[serde(flatten)]
    pub kind: StreamEventKind,
}

/// Clean user-facing event for WebSocket and outbound channels.
///
/// This is the data-plane event type — it contains only what the end-user
/// should see. Control-plane events (TokenStats, Subagent lifecycle) are
/// intentionally excluded and handled internally via `SystemEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Thinking/reasoning content from the LLM
    Thinking { content: Arc<str> },

    /// A tool call has started
    ToolStart {
        name: Arc<str>,
        #[serde(default)]
        arguments: Option<Arc<str>>,
    },

    /// A tool call has completed
    ToolEnd {
        name: Arc<str>,
        #[serde(default)]
        output: Option<Arc<str>>,
    },

    /// Streaming content chunk
    Content { content: Arc<str> },

    /// Stream has completed for this iteration
    Done,

    /// Plain text message (legacy support for non-streaming channels)
    Text { content: Arc<str> },

    /// Error message
    Error { message: Arc<str> },

    /// Context usage statistics
    ContextStats {
        token_budget: usize,
        compaction_threshold: f64,
        threshold_tokens: usize,
        current_tokens: usize,
        usage_percent: f64,
        is_compressing: bool,
    },

    /// Watermark and sequence information
    WatermarkInfo {
        watermark: i64,
        max_sequence: i64,
        uncompacted_count: usize,
        compacted_percent: f64,
    },
}

impl ChatEvent {
    /// Create a content message
    pub fn content(content: impl Into<String>) -> Self {
        Self::Content {
            content: Arc::from(content.into()),
        }
    }

    /// Create a thinking message
    pub fn thinking(content: impl Into<String>) -> Self {
        Self::Thinking {
            content: Arc::from(content.into()),
        }
    }

    /// Create a tool_start message
    pub fn tool_start(name: impl Into<String>, arguments: Option<String>) -> Self {
        Self::ToolStart {
            name: Arc::from(name.into()),
            arguments: arguments.map(Arc::from),
        }
    }

    /// Create a tool_end message
    pub fn tool_end(name: impl Into<String>, output: Option<String>) -> Self {
        Self::ToolEnd {
            name: Arc::from(name.into()),
            output: output.map(Arc::from),
        }
    }

    /// Create a done message
    pub fn done() -> Self {
        Self::Done
    }

    /// Create a text message
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            content: Arc::from(content.into()),
        }
    }

    /// Create an error message
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: Arc::from(message.into()),
        }
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"type":"text","content":"serialization error"}"#.to_string())
    }
}

/// WebSocket message type — a clean alias for `ChatEvent`.
pub type WebSocketMessage = ChatEvent;

impl StreamEvent {
    // === Main Agent Event Constructors ===

    /// Create a thinking message for the main agent
    pub fn thinking(content: impl Into<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::Thinking {
                content: Arc::from(content.into()),
            },
        }
    }

    /// Create a tool_start message for the main agent
    pub fn tool_start(name: impl Into<String>, arguments: Option<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::ToolStart {
                name: Arc::from(name.into()),
                arguments: arguments.map(Arc::from),
            },
        }
    }

    /// Create a tool_end message for the main agent
    pub fn tool_end(name: impl Into<String>, output: Option<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::ToolEnd {
                name: Arc::from(name.into()),
                output: output.map(Arc::from),
            },
        }
    }

    /// Create a content message for the main agent
    pub fn content(content: impl Into<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::Content {
                content: Arc::from(content.into()),
            },
        }
    }

    /// Create a done message for the main agent
    pub fn done() -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::Done,
        }
    }

    /// Create a plain text message (legacy)
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            agent_id: None,
            kind: StreamEventKind::Text {
                content: Arc::from(content.into()),
            },
        }
    }

    // === Subagent Event Constructors ===

    /// Create a thinking message for a subagent
    pub fn subagent_thinking(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            agent_id: Some(Arc::from(id.into())),
            kind: StreamEventKind::Thinking {
                content: Arc::from(content.into()),
            },
        }
    }

    /// Create a tool_start message for a subagent
    pub fn subagent_tool_start(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Option<String>,
    ) -> Self {
        Self {
            agent_id: Some(Arc::from(id.into())),
            kind: StreamEventKind::ToolStart {
                name: Arc::from(name.into()),
                arguments: arguments.map(Arc::from),
            },
        }
    }

    /// Create a tool_end message for a subagent
    pub fn subagent_tool_end(
        id: impl Into<String>,
        name: impl Into<String>,
        output: Option<String>,
    ) -> Self {
        Self {
            agent_id: Some(Arc::from(id.into())),
            kind: StreamEventKind::ToolEnd {
                name: Arc::from(name.into()),
                output: output.map(Arc::from),
            },
        }
    }

    /// Create a content message for a subagent
    pub fn subagent_content(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            agent_id: Some(Arc::from(id.into())),
            kind: StreamEventKind::Content {
                content: Arc::from(content.into()),
            },
        }
    }

    // === Utility Methods ===

    /// Get the agent_id if this is a subagent event
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// Check if this event is from a subagent
    pub fn is_subagent_event(&self) -> bool {
        self.agent_id.is_some()
    }

    /// Check if this is a main agent event
    pub fn is_main_agent_event(&self) -> bool {
        self.agent_id.is_none()
    }

    /// Inject a subagent ID into this event.
    pub fn with_agent_id(mut self, id: Arc<str>) -> Self {
        self.agent_id = Some(id);
        self
    }

    /// Convert to a user-facing `ChatEvent` if this is a main-agent data event.
    ///
    /// Returns `None` for subagent events (anything with `agent_id` set).
    pub fn to_chat_event(&self) -> Option<ChatEvent> {
        if self.agent_id.is_some() {
            return None;
        }
        Some(match &self.kind {
            StreamEventKind::Thinking { content } => ChatEvent::Thinking {
                content: Arc::clone(content),
            },
            StreamEventKind::ToolStart { name, arguments } => ChatEvent::ToolStart {
                name: Arc::clone(name),
                arguments: arguments.as_ref().map(Arc::clone),
            },
            StreamEventKind::ToolEnd { name, output } => ChatEvent::ToolEnd {
                name: Arc::clone(name),
                output: output.as_ref().map(Arc::clone),
            },
            StreamEventKind::Content { content } => ChatEvent::Content {
                content: Arc::clone(content),
            },
            StreamEventKind::Done => ChatEvent::Done,
            StreamEventKind::Text { content } => ChatEvent::Text {
                content: Arc::clone(content),
            },
        })
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
    fn test_main_agent_events() {
        let thinking = StreamEvent::thinking("test");
        assert!(thinking.agent_id().is_none());
        assert!(thinking.is_main_agent_event());

        let content = StreamEvent::content("hello");
        assert!(content.agent_id().is_none());
        assert!(!content.is_subagent_event());

        let done = StreamEvent::done();
        assert!(done.agent_id().is_none());
    }

    #[test]
    fn test_subagent_events() {
        let thinking = StreamEvent::subagent_thinking("uuid-123", "test");
        assert_eq!(thinking.agent_id(), Some("uuid-123"));
        assert!(thinking.is_subagent_event());

        let content = StreamEvent::subagent_content("uuid-123", "hello");
        assert_eq!(content.agent_id(), Some("uuid-123"));
    }

    #[test]
    fn test_subagent_started_serialization() {
        let msg = InternalSignal::subagent_started("id-123", "Search docs", 1);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"subagent_started"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""task":"Search docs"#));
        assert!(json.contains(r#""index":1"#));
    }

    #[test]
    fn test_subagent_thinking_serialization() {
        let msg = StreamEvent::subagent_thinking("id-123", "Analyzing...");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"thinking"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""content":"Analyzing..."#));
    }

    #[test]
    fn test_subagent_completed_serialization() {
        let msg = InternalSignal::subagent_completed("id-123", 1, "Done", 5);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"subagent_completed"#));
        assert!(json.contains(r#""tool_count":5"#));
    }

    #[test]
    fn test_subagent_message_deserialization() {
        let json =
            r#"{"type":"subagent_started","agent_id":"id-123","task":"Test task","index":1}"#;
        let msg: InternalSignal = serde_json::from_str(json).unwrap();
        match msg {
            InternalSignal::SubagentStarted {
                agent_id,
                task,
                index,
            } => {
                assert_eq!(agent_id.as_ref(), "id-123");
                assert_eq!(task.as_ref(), "Test task");
                assert_eq!(index, 1);
            }
            _ => panic!("Expected SubagentStarted"),
        }
    }

    #[test]
    fn test_subagent_tool_messages() {
        let start_msg = StreamEvent::subagent_tool_start(
            "id-123",
            "read_file",
            Some(r#"{"path":"/test.txt"}"#.to_string()),
        );
        let json = start_msg.to_json();
        assert!(json.contains(r#""type":"tool_start"#));
        assert!(json.contains(r#""agent_id":"id-123"#));
        assert!(json.contains(r#""name":"read_file"#));

        let end_msg = StreamEvent::subagent_tool_end(
            "id-123",
            "read_file",
            Some("file contents".to_string()),
        );
        let json = end_msg.to_json();
        assert!(json.contains(r#""type":"tool_end"#));
        assert!(json.contains(r#""output":"file contents"#));
    }

    #[test]
    fn test_token_stats_event() {
        let stats = InternalSignal::token_stats(1000, 500, 1500, 0.01, "USD");
        match stats {
            InternalSignal::TokenStats {
                agent_id,
                input_tokens,
                output_tokens,
                total_tokens,
                cost,
                currency,
            } => {
                assert!(agent_id.is_none());
                assert_eq!(input_tokens, 1000);
                assert_eq!(output_tokens, 500);
                assert_eq!(total_tokens, 1500);
                assert!((cost - 0.01).abs() < 0.0001);
                assert_eq!(currency.as_ref(), "USD");
            }
            _ => panic!("Expected TokenStats"),
        }
    }

    #[test]
    fn test_websocket_message_is_chat_event() {
        let msg: WebSocketMessage = ChatEvent::thinking("test");
        assert!(matches!(msg, ChatEvent::Thinking { .. }));
    }
}
