//! Stream processing utilities for the kernel.
//!
//! Provides streaming event types and native async Stream support.
//! Uses the unified `StreamEvent` from `gasket_types` - no local event type.

use anyhow::Result;
use futures_util::stream::Stream;
use futures_util::StreamExt;
use std::collections::BTreeMap;
use tracing::{debug, trace};

use gasket_providers::{parse_json_args, ChatResponse, ToolCall, ToolCallDelta};
pub use gasket_types::StreamEvent;

/// Accumulates streamed tool-call deltas into complete `ToolCall` objects.
pub struct ToolCallAccumulator {
    pending: BTreeMap<usize, PartialToolCall>,
}

struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
        }
    }

    pub fn feed(&mut self, delta: &ToolCallDelta) {
        let entry = self
            .pending
            .entry(delta.index)
            .or_insert_with(|| PartialToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });

        if let Some(ref id) = delta.id {
            entry.id = id.clone();
        }
        if let Some(ref name) = delta.function_name {
            entry.name = name.clone();
        }
        if let Some(ref args) = delta.function_arguments {
            entry.arguments.push_str(args);
        }
    }

    pub fn finalize(self) -> Vec<ToolCall> {
        self.pending
            .into_values()
            .map(|partial| {
                let arguments = parse_json_args(&partial.arguments);
                ToolCall::new(partial.id, partial.name, arguments)
            })
            .collect()
    }
}

impl Default for ToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper: convert empty String to None.
fn optional_string(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Accumulator for streaming response data.
struct StreamAccumulator {
    content: String,
    reasoning_content: String,
    tool_acc: ToolCallAccumulator,
    accumulated_usage: Option<gasket_providers::Usage>,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self {
            content: String::new(),
            reasoning_content: String::new(),
            tool_acc: ToolCallAccumulator::new(),
            accumulated_usage: None,
        }
    }

    fn feed(&mut self, chunk: &gasket_providers::ChatStreamChunk) {
        if let Some(ref text) = chunk.delta.content {
            if !text.is_empty() {
                self.content.push_str(text);
            }
        }
        if let Some(ref reasoning) = chunk.delta.reasoning_content {
            if !reasoning.is_empty() {
                self.reasoning_content.push_str(reasoning);
            }
        }
        for tc_delta in &chunk.delta.tool_calls {
            self.tool_acc.feed(tc_delta);
        }
        if let Some(ref usage) = chunk.usage {
            self.accumulated_usage = Some(usage.clone());
        }
    }

    fn finalize(self) -> ChatResponse {
        ChatResponse {
            content: optional_string(self.content),
            tool_calls: self.tool_acc.finalize(),
            reasoning_content: optional_string(self.reasoning_content),
            usage: self.accumulated_usage,
        }
    }
}

/// Convert LLM stream to event stream with backpressure support.
pub fn stream_events(
    llm_stream: gasket_providers::ChatStream,
) -> (
    impl Stream<Item = StreamEvent>,
    impl std::future::Future<Output = Result<ChatResponse>>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(32);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel::<Result<ChatResponse>>();

    let handle = tokio::spawn(async move {
        debug!("[StreamEvents] Producer task started");
        let mut llm_stream = llm_stream;
        let mut acc = StreamAccumulator::new();
        let mut chunk_count = 0usize;
        let mut closed = false;

        while let Some(chunk_result) = llm_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    chunk_count += 1;
                    if chunk_count == 1 {
                        debug!("[StreamEvents] Received first chunk from LLM");
                    }
                    trace!(
                        "[StreamEvents] Chunk {}: content={:?}, tool_calls={}",
                        chunk_count,
                        chunk.delta.content,
                        chunk.delta.tool_calls.len()
                    );

                    // Extract deltas before feeding to accumulator
                    let content_delta = chunk.delta.content.clone().filter(|s| !s.is_empty());
                    let reasoning_delta = chunk
                        .delta
                        .reasoning_content
                        .clone()
                        .filter(|s| !s.is_empty());

                    acc.feed(&chunk);

                    if !closed {
                        if let Some(text) = content_delta {
                            if tx.send(StreamEvent::content(text)).await.is_err() {
                                debug!(
                                    "[StreamEvents] Channel closed, aborting event send after {} chunks",
                                    chunk_count
                                );
                                closed = true;
                            }
                        }
                        if !closed {
                            if let Some(reasoning) = reasoning_delta {
                                if tx.send(StreamEvent::thinking(reasoning)).await.is_err() {
                                    debug!("[StreamEvents] Channel closed during reasoning");
                                    closed = true;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("[StreamEvents] Error in stream: {}", e);
                    let _ = response_tx.send(Err(e.into()));
                    return;
                }
            }
        }

        debug!(
            "[StreamEvents] Stream completed, {} chunks, content len: {}",
            chunk_count,
            acc.content.len()
        );

        let _ = response_tx.send(Ok(acc.finalize()));
    });

    let event_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let response_future = async move {
        response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Producer task died"))?
    };

    (event_stream, response_future, handle)
}

/// Collect LLM stream into a response without emitting events.
pub async fn collect_stream_response(
    mut llm_stream: gasket_providers::ChatStream,
) -> Result<ChatResponse> {
    let mut acc = StreamAccumulator::new();

    while let Some(chunk_result) = llm_stream.next().await {
        let chunk = chunk_result?;
        acc.feed(&chunk);
    }

    Ok(acc.finalize())
}

/// Buffered events for a single subagent or agent execution.
#[derive(Debug, Default)]
pub struct BufferedEvents {
    pub messages: Vec<gasket_types::WebSocketMessage>,
    pub completed: bool,
}

impl BufferedEvents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: gasket_types::WebSocketMessage) {
        self.messages.push(message);
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn flush(&mut self) -> Vec<gasket_types::WebSocketMessage> {
        std::mem::take(&mut self.messages)
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.completed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_accumulator_empty() {
        let acc = ToolCallAccumulator::new();
        let calls = acc.finalize();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_tool_call_accumulator_single() {
        let mut acc = ToolCallAccumulator::new();
        acc.feed(&ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            function_name: Some("test_tool".to_string()),
            function_arguments: Some(r#"{"arg":"value"}"#.to_string()),
        });

        let calls = acc.finalize();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "test_tool");
    }

    #[test]
    fn test_tool_call_accumulator_multiple() {
        let mut acc = ToolCallAccumulator::new();

        acc.feed(&ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            function_name: Some("tool_a".to_string()),
            function_arguments: None,
        });
        acc.feed(&ToolCallDelta {
            index: 1,
            id: Some("call_2".to_string()),
            function_name: Some("tool_b".to_string()),
            function_arguments: None,
        });
        acc.feed(&ToolCallDelta {
            index: 0,
            id: None,
            function_name: None,
            function_arguments: Some(r#"{"x":1}"#.to_string()),
        });

        let calls = acc.finalize();
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn test_tool_call_accumulator_sorts_by_index() {
        let mut acc = ToolCallAccumulator::new();

        acc.feed(&ToolCallDelta {
            index: 10,
            id: Some("call_2".to_string()),
            function_name: Some("tool_a".to_string()),
            function_arguments: Some(r#"{"x":1}"#.to_string()),
        });
        acc.feed(&ToolCallDelta {
            index: 2,
            id: Some("call_10".to_string()),
            function_name: Some("tool_b".to_string()),
            function_arguments: Some(r#"{"y":2}"#.to_string()),
        });

        let calls = acc.finalize();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "tool_b"); // index 2
        assert_eq!(calls[1].function.name, "tool_a"); // index 10
    }

    #[test]
    fn test_empty_buffer() {
        let mut buffer = BufferedEvents::new();
        let flushed = buffer.flush();
        assert!(flushed.is_empty());
    }

    #[test]
    fn test_buffer_clear() {
        let mut buffer = BufferedEvents::new();
        buffer.push(gasket_types::WebSocketMessage::content("test"));
        buffer.completed = true;

        buffer.clear();

        assert!(buffer.is_empty());
        assert!(!buffer.completed);
    }
}
