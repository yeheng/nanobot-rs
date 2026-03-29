//! SSE (Server-Sent Events) parsing utilities for LLM streaming responses

use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;
use tracing::{debug, trace};

use crate::base::{ChatStreamChunk, ChatStreamDelta, FinishReason, ToolCallDelta};
use crate::ProviderError;

/// Parse a raw SSE byte stream into `ChatStreamChunk`s.
///
/// The input is a `Stream<Item = Result<Bytes>>` obtained from
/// `reqwest::Response::bytes_stream()`. Each SSE event looks like:
///
/// ```text
/// data: {"id":"...","choices":[{"delta":{...}}]}
///
/// ```
///
/// The stream terminates on `data: [DONE]`.
pub fn parse_sse_stream(
    byte_stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<ChatStreamChunk, ProviderError>> + Send + 'static {
    // SSE events can be split across multiple byte chunks. We accumulate a
    // buffer and split on double-newline boundaries.
    let lines_stream = sse_lines(byte_stream);

    lines_stream.filter_map(|line_result| async move {
        match line_result {
            Err(e) => Some(Err(ProviderError::NetworkError(format!(
                "SSE stream error: {}",
                e
            )))),
            Ok(line) => {
                // Skip empty lines and comments
                let line = line.trim();
                if line.is_empty() || line.starts_with(':') {
                    return None;
                }

                // Parse "data: ..." lines
                let data = line.strip_prefix("data: ")?;

                // Check for stream terminator
                if data.trim() == "[DONE]" {
                    return None;
                }

                // Parse the JSON chunk
                match serde_json::from_str::<OpenAIStreamChunk>(data) {
                    Ok(chunk) => {
                        let converted = convert_chunk(chunk);
                        Some(Ok(converted))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse SSE chunk: {} | data: {}", e, data);
                        None
                    }
                }
            }
        }
    })
}

/// Convert a raw SSE byte stream into individual lines.
///
/// SSE events are separated by `\n\n`. Individual fields within an event are
/// separated by `\n`. We yield each non-empty line.
///
/// This is a shared utility for all SSE-based streaming providers.
///
/// Uses `Vec<u8>` buffering to correctly handle multi-byte UTF-8 characters
/// that may be split across network chunks.
pub fn sse_lines(
    byte_stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<String, anyhow::Error>> + Send + 'static {
    debug!("[SSE] Starting to parse byte stream");

    futures::stream::unfold(
        (byte_stream.boxed(), Vec::<u8>::new(), false),
        |(mut stream, mut buffer, mut received_data)| async move {
            loop {
                // Try to find a newline in the byte buffer
                if let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                    // Extract the line bytes
                    let line_bytes: Vec<u8> = buffer.drain(..=newline_pos).collect();
                    // Convert to string (remove the newline)
                    match String::from_utf8(line_bytes) {
                        Ok(line) => {
                            let line = line.trim_end().to_string();
                            if !line.is_empty() {
                                trace!("[SSE] Received line: {}", &line);
                                return Some((Ok(line), (stream, buffer, received_data)));
                            }
                            // Skip empty lines, continue looking
                        }
                        Err(e) => {
                            return Some((
                                Err(anyhow::anyhow!("Invalid UTF-8 in stream: {}", e)),
                                (stream, buffer, received_data),
                            ));
                        }
                    }
                    continue;
                }

                // Need more data
                trace!("[SSE] Waiting for more data from stream...");
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        if !received_data {
                            received_data = true;
                            debug!("[SSE] Received first chunk: {} bytes", bytes.len());
                        }
                        trace!("[SSE] Received chunk: {} bytes", bytes.len());
                        buffer.extend_from_slice(&bytes);
                    }
                    Some(Err(e)) => {
                        debug!("[SSE] Stream error: {}", e);
                        return Some((
                            Err(anyhow::anyhow!("Stream error: {}", e)),
                            (stream, buffer, received_data),
                        ));
                    }
                    None => {
                        debug!("[SSE] Stream ended, buffer size: {}", buffer.len());
                        // Stream ended; yield any remaining data
                        if !buffer.is_empty() {
                            let remaining = std::mem::take(&mut buffer);
                            match String::from_utf8(remaining) {
                                Ok(line) if !line.trim().is_empty() => {
                                    debug!("[SSE] Yielding remaining data: {} bytes", line.len());
                                    return Some((
                                        Ok(line.trim_end().to_string()),
                                        (stream, buffer, received_data),
                                    ));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    return Some((
                                        Err(anyhow::anyhow!("Invalid UTF-8 in stream: {}", e)),
                                        (stream, buffer, received_data),
                                    ));
                                }
                            }
                        }
                        return None;
                    }
                }
            }
        },
    )
}

/// Convert an OpenAI-format stream chunk into our internal type.
fn convert_chunk(chunk: OpenAIStreamChunk) -> ChatStreamChunk {
    let choice = match chunk.choices.into_iter().next() {
        Some(c) => c,
        None => {
            return ChatStreamChunk {
                delta: ChatStreamDelta::default(),
                finish_reason: None,
                usage: None,
            }
        }
    };

    let finish_reason = choice
        .finish_reason
        .as_deref()
        .map(FinishReason::from_api_str);

    let tool_calls = choice
        .delta
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| ToolCallDelta {
            index: tc.index,
            id: tc.id,
            function_name: tc.function.as_ref().and_then(|f| f.name.clone()),
            function_arguments: tc.function.as_ref().and_then(|f| f.arguments.clone()),
        })
        .collect();

    // Convert usage if present (typically in the final chunk)
    let usage = chunk.usage.map(|u| crate::Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
    });

    ChatStreamChunk {
        delta: ChatStreamDelta {
            content: choice.delta.content,
            reasoning_content: choice.delta.reasoning_content,
            tool_calls,
        },
        finish_reason,
        usage,
    }
}

// ---------------------------------------------------------------------------
// OpenAI streaming API types (for deserialization only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    choices: Vec<OpenAIStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAIStreamUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamUsage {
    #[serde(default, rename = "prompt_tokens")]
    input_tokens: usize,
    #[serde(default, rename = "completion_tokens")]
    output_tokens: usize,
    #[serde(default, rename = "total_tokens")]
    total_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<OpenAIStreamFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_text_chunk() {
        let raw = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);
        assert_eq!(result.delta.content.as_deref(), Some("Hello"));
        assert!(result.finish_reason.is_none());
    }

    #[test]
    fn test_convert_finish_chunk() {
        let raw =
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);
        assert_eq!(result.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn test_convert_tool_call_chunk() {
        let raw = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);
        assert_eq!(result.delta.tool_calls.len(), 1);
        assert_eq!(result.delta.tool_calls[0].id.as_deref(), Some("call_abc"));
        assert_eq!(
            result.delta.tool_calls[0].function_name.as_deref(),
            Some("read_file")
        );
    }

    #[test]
    fn test_convert_reasoning_chunk() {
        let raw = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"reasoning_content":"Let me think..."},"finish_reason":null}]}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);
        assert_eq!(
            result.delta.reasoning_content.as_deref(),
            Some("Let me think...")
        );
    }

    #[test]
    fn test_convert_chunk_with_usage() {
        // This is the format OpenAI uses in the final streaming chunk
        let raw = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);

        assert_eq!(result.finish_reason, Some(FinishReason::Stop));
        assert!(result.usage.is_some());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_convert_chunk_without_usage() {
        // Chunks without usage should have usage = None
        let raw = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(raw).unwrap();
        let result = convert_chunk(chunk);
        assert!(result.usage.is_none());
    }
}
