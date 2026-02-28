//! Stream processing utilities for the agent.
//!
//! Provides streaming event types and stream accumulation logic.

use std::collections::HashMap;

use anyhow::Result;
use futures::StreamExt;

use crate::providers::{parse_json_args, ChatResponse, ToolCall, ToolCallDelta};

/// Callback type for streaming output.
///
/// Called for each chunk of text or reasoning content as it arrives.
pub type StreamCallback = Box<dyn Fn(&StreamEvent) + Send + Sync>;

/// Events emitted during streaming.
#[derive(Debug)]
pub enum StreamEvent {
    /// Incremental text content
    Content(String),
    /// Incremental reasoning/thinking content
    Reasoning(String),
    /// A tool is being called
    ToolStart { name: String },
    /// Tool execution finished
    ToolEnd { name: String, output: String },
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

/// Consume a stream, emitting events via callback, and return the
/// accumulated complete `ChatResponse`.
pub async fn accumulate_stream(
    stream: &mut crate::providers::ChatStream,
    callback: &StreamCallback,
) -> Result<ChatResponse> {
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_acc = ToolCallAccumulator::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;

        // Accumulate text content
        if let Some(ref text) = chunk.delta.content {
            if !text.is_empty() {
                content.push_str(text);
                callback(&StreamEvent::Content(text.clone()));
            }
        }

        // Accumulate reasoning content
        if let Some(ref reasoning) = chunk.delta.reasoning_content {
            if !reasoning.is_empty() {
                reasoning_content.push_str(reasoning);
                callback(&StreamEvent::Reasoning(reasoning.clone()));
            }
        }

        // Accumulate tool call deltas
        for tc_delta in &chunk.delta.tool_calls {
            tool_acc.feed(tc_delta);
        }
    }

    eprintln!();

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
    })
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
