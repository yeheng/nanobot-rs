//! Stream processing utilities for the agent.
//!
//! Provides streaming event types and native async Stream support.

use std::collections::HashMap;

use anyhow::Result;
use futures::stream::Stream;
use futures::StreamExt;
use tracing::{debug, trace};

use gasket_providers::{parse_json_args, ChatResponse, ToolCall, ToolCallDelta};

/// Events emitted during streaming.
#[derive(Debug)]
pub enum StreamEvent {
    /// Incremental text content
    Content(String),
    /// Incremental reasoning/thinking content
    Reasoning(String),
    /// A tool is being called
    ToolStart {
        name: String,
        /// Tool arguments as JSON string (optional)
        arguments: Option<String>,
    },
    /// Tool execution finished
    ToolEnd { name: String, output: String },
    /// Token usage statistics (emitted when stream completes)
    TokenStats {
        /// Input tokens
        input_tokens: usize,
        /// Output tokens
        output_tokens: usize,
        /// Total tokens
        total_tokens: usize,
        /// Cost (if pricing configured)
        cost: f64,
        /// Currency code
        currency: String,
    },
    /// Stream completed
    Done,
}

/// Accumulates streamed tool-call deltas into complete `ToolCall` objects.
///
/// Streaming APIs send tool calls as incremental fragments across multiple
/// chunks. This struct reassembles them by tracking `(id, name, arguments)`
/// per tool-call index.
pub struct ToolCallAccumulator {
    /// index → (id, name, arguments_buffer)
    pending: HashMap<usize, PartialToolCall>,
}

/// A tool call that is still being assembled from stream deltas.
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    /// Create a new empty accumulator.
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Feed a single delta into the accumulator.
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

    /// Finalize all pending tool calls into sorted `ToolCall` objects.
    pub fn finalize(self) -> Vec<ToolCall> {
        let mut calls: Vec<ToolCall> = self
            .pending
            .into_values()
            .map(|partial| {
                let arguments = parse_json_args(&partial.arguments);
                ToolCall::new(partial.id, partial.name, arguments)
            })
            .collect();
        calls.sort_by_key(|tc| tc.id.clone());
        calls
    }
}

impl Default for ToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Accumulator for streaming response data.
///
/// Eliminates duplication between `stream_events()` and `collect_stream_response()`.
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

    fn finalize(self) -> gasket_providers::ChatResponse {
        gasket_providers::ChatResponse {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls: self.tool_acc.finalize(),
            reasoning_content: if self.reasoning_content.is_empty() {
                None
            } else {
                Some(self.reasoning_content)
            },
            usage: self.accumulated_usage,
        }
    }
}

/// Convert LLM stream to event stream with backpressure support.
///
/// Returns (event_stream, final_response_handle).
///
/// The internal producer task is spawned immediately to consume the LLM stream
/// and send events to the channel. The event_stream receives these events,
/// and final_response_handle can be awaited to get the complete response.
pub fn stream_events(
    llm_stream: gasket_providers::ChatStream,
) -> (
    impl Stream<Item = StreamEvent>,
    impl std::future::Future<Output = Result<ChatResponse>>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(32);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel::<Result<ChatResponse>>();

    // Spawn the producer task immediately to consume the LLM stream
    tokio::spawn(async move {
        debug!("[StreamEvents] Producer task started, consuming LLM stream");
        let mut llm_stream = llm_stream;
        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut tool_acc = ToolCallAccumulator::new();
        let mut accumulated_usage = None;
        let mut chunk_count = 0usize;

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

                    if let Some(ref text) = chunk.delta.content {
                        if !text.is_empty() {
                            content.push_str(text);
                            // Check if channel is still open before continuing
                            // If closed, abort the LLM stream to save resources
                            if tx.try_send(StreamEvent::Content(text.clone())).is_err() {
                                debug!(
                                    "[StreamEvents] Channel closed, aborting LLM stream after {} chunks",
                                    chunk_count
                                );
                                // Channel closed - client disconnected
                                // Return partial response to avoid wasting LLM tokens
                                let _ = response_tx.send(Ok(ChatResponse {
                                    content: Some(content),
                                    tool_calls: tool_acc.finalize(),
                                    reasoning_content: if reasoning_content.is_empty() {
                                        None
                                    } else {
                                        Some(reasoning_content)
                                    },
                                    usage: accumulated_usage,
                                }));
                                return;
                            }
                        }
                    }

                    if let Some(ref reasoning) = chunk.delta.reasoning_content {
                        if !reasoning.is_empty() {
                            reasoning_content.push_str(reasoning);
                            // Check if channel is still open
                            if tx
                                .try_send(StreamEvent::Reasoning(reasoning.clone()))
                                .is_err()
                            {
                                debug!(
                                    "[StreamEvents] Channel closed during reasoning, aborting LLM stream"
                                );
                                let _ = response_tx.send(Ok(ChatResponse {
                                    content: Some(content),
                                    tool_calls: tool_acc.finalize(),
                                    reasoning_content: if reasoning_content.is_empty() {
                                        None
                                    } else {
                                        Some(reasoning_content)
                                    },
                                    usage: accumulated_usage,
                                }));
                                return;
                            }
                        }
                    }

                    for tc_delta in &chunk.delta.tool_calls {
                        tool_acc.feed(tc_delta);
                    }

                    if let Some(ref usage) = chunk.usage {
                        accumulated_usage = Some(usage.clone());
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
            "[StreamEvents] Stream completed, received {} chunks, content length: {}",
            chunk_count,
            content.len()
        );
        // NOTE: Do NOT send Done here! Done should only be sent when the entire
        // agent loop completes (no more tool calls). The executor_core.rs is
        // responsible for sending Done at the right time.

        let response = ChatResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls: tool_acc.finalize(),
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
            usage: accumulated_usage,
        };

        let _ = response_tx.send(Ok(response));
    });

    let event_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let response_future = async move {
        response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Producer task died"))?
    };

    (event_stream, response_future)
}

/// Collect LLM stream into a response without emitting events.
///
/// Use this instead of `stream_events()` when you don't need streaming callbacks.
/// This avoids the channel overhead and potential deadlock issues.
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

        // Feed deltas for multiple tool calls (may arrive interleaved)
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
}
