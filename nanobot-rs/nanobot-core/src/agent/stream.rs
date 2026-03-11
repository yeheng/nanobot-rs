//! Stream processing utilities for the agent.
//!
//! Provides streaming event types and native async Stream support.

use std::collections::HashMap;

use anyhow::Result;
use futures::stream::Stream;
use futures::StreamExt;

use crate::providers::{parse_json_args, ChatResponse, ToolCall, ToolCallDelta};

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

/// Convert LLM stream to event stream with backpressure support.
///
/// Returns (event_stream, final_response_future).
pub fn stream_events(
    mut llm_stream: crate::providers::ChatStream,
) -> (
    impl Stream<Item = StreamEvent>,
    impl std::future::Future<Output = Result<ChatResponse>>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(32);

    let response_future = async move {
        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut tool_acc = ToolCallAccumulator::new();
        let mut accumulated_usage = None;

        while let Some(chunk_result) = llm_stream.next().await {
            let chunk = chunk_result?;

            if let Some(ref text) = chunk.delta.content {
                if !text.is_empty() {
                    content.push_str(text);
                    let _ = tx.send(StreamEvent::Content(text.clone())).await;
                }
            }

            if let Some(ref reasoning) = chunk.delta.reasoning_content {
                if !reasoning.is_empty() {
                    reasoning_content.push_str(reasoning);
                    let _ = tx.send(StreamEvent::Reasoning(reasoning.clone())).await;
                }
            }

            for tc_delta in &chunk.delta.tool_calls {
                tool_acc.feed(tc_delta);
            }

            if let Some(ref usage) = chunk.usage {
                accumulated_usage = Some(usage.clone());
            }
        }

        let _ = tx.send(StreamEvent::Done).await;

        Ok(ChatResponse {
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
        })
    };

    (
        tokio_stream::wrappers::ReceiverStream::new(rx),
        response_future,
    )
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
