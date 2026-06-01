//! Type conversion bridge between gasket provider types and rig-core types.

use futures_util::stream::StreamExt;
use rig::completion::{
    CompletionError, CompletionRequest, CompletionResponse, GetTokenUsage, Message as RigMessage,
    ToolDefinition as RigToolDefinition,
};
use rig::message::{
    AssistantContent, Reasoning, ReasoningContent, ToolCall as RigToolCall,
    ToolFunction as RigToolFunction,
};
use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};
use rig::OneOrMany;

use crate::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamChunk, ChatStreamDelta,
    FinishReason, FunctionCall, MessageRole, ProviderError, ToolCall, ToolCallDelta, Usage,
};

/// Convert gasket ChatMessage to rig Message
pub fn to_rig_message(msg: ChatMessage) -> RigMessage {
    match msg.role {
        MessageRole::System => RigMessage::system(msg.content.unwrap_or_default()),
        MessageRole::User => RigMessage::user(msg.content.unwrap_or_default()),
        MessageRole::Assistant => {
            let mut contents: Vec<AssistantContent> = Vec::new();

            // Add reasoning content first if present
            if let Some(reasoning) = msg.reasoning_content {
                contents.push(AssistantContent::Reasoning(Reasoning::new(&reasoning)));
            }

            // Add tool calls if present
            if let Some(tool_calls) = msg.tool_calls {
                for tc in tool_calls {
                    contents.push(AssistantContent::ToolCall(RigToolCall::new(
                        tc.id,
                        RigToolFunction {
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        },
                    )));
                }
            }

            // Add text content if present
            if let Some(content) = msg.content {
                contents.push(AssistantContent::text(content));
            }

            if contents.is_empty() {
                RigMessage::assistant(String::new())
            } else {
                RigMessage::Assistant {
                    id: None,
                    content: OneOrMany::many(contents)
                        .unwrap_or_else(|_| OneOrMany::one(AssistantContent::text(String::new()))),
                }
            }
        }
        MessageRole::Tool => RigMessage::tool_result(
            msg.tool_call_id.unwrap_or_default(),
            msg.content.unwrap_or_default(),
        ),
    }
}

/// Convert gasket ChatRequest to rig CompletionRequest
pub fn to_rig_request(request: ChatRequest) -> CompletionRequest {
    let mut messages: Vec<RigMessage> = request.messages.into_iter().map(to_rig_message).collect();

    // Extract system message as preamble if present
    let preamble = if let Some(first) = messages.first() {
        if matches!(first, RigMessage::System { .. }) {
            let system_msg = messages.remove(0);
            match system_msg {
                RigMessage::System { content } => Some(content),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };

    CompletionRequest {
        model: Some(request.model),
        preamble,
        chat_history: OneOrMany::many(messages)
            .unwrap_or_else(|_| OneOrMany::one(RigMessage::user(""))),
        documents: vec![],
        tools: request
            .tools
            .map(|tools| {
                tools
                    .into_iter()
                    .map(|t| RigToolDefinition {
                        name: t.function.name,
                        description: t.function.description,
                        parameters: t.function.parameters,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        temperature: request.temperature.map(|t| t as f64),
        max_tokens: request.max_tokens.map(|t| t as u64),
        tool_choice: None,
        additional_params: None,
        output_schema: None,
    }
}

/// Convert rig CompletionResponse to gasket ChatResponse
pub fn from_rig_response<T>(response: CompletionResponse<T>) -> ChatResponse {
    let mut content = None;
    let mut tool_calls = Vec::new();
    let mut reasoning_content = None;

    for item in response.choice.into_iter() {
        match item {
            AssistantContent::Text(text) => {
                content = Some(text.text);
            }
            AssistantContent::ToolCall(tc) => {
                tool_calls.push(ToolCall {
                    id: tc.id,
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                });
            }
            AssistantContent::Reasoning(reasoning) => {
                reasoning_content = Some(
                    reasoning
                        .content
                        .iter()
                        .map(|r| match r {
                            ReasoningContent::Text { text, .. } => text.clone(),
                            _ => String::new(),
                        })
                        .collect::<String>(),
                );
            }
            _ => {}
        }
    }

    ChatResponse {
        content,
        tool_calls,
        reasoning_content,
        usage: Some(Usage {
            input_tokens: response.usage.input_tokens as usize,
            output_tokens: response.usage.output_tokens as usize,
            total_tokens: response.usage.total_tokens as usize,
        }),
    }
}

/// Convert rig streaming response to gasket ChatStream
pub fn from_rig_stream<S, R>(stream: S) -> ChatStream
where
    S: futures_util::Stream<Item = Result<StreamedAssistantContent<R>, CompletionError>>
        + Send
        + 'static,
    R: GetTokenUsage + Send + 'static,
{
    let mapped = stream.map(|result| match result {
        Ok(content) => {
            let chunk = match content {
                StreamedAssistantContent::Text(text) => ChatStreamChunk {
                    delta: ChatStreamDelta {
                        content: Some(text.text),
                        reasoning_content: None,
                        tool_calls: vec![],
                    },
                    finish_reason: None,
                    usage: None,
                },
                StreamedAssistantContent::ToolCallDelta { id, content, .. } => {
                    let (function_name, function_arguments) = match content {
                        ToolCallDeltaContent::Name(name) => (Some(name), None),
                        ToolCallDeltaContent::Delta(delta) => (None, Some(delta)),
                    };
                    ChatStreamChunk {
                        delta: ChatStreamDelta {
                            content: None,
                            reasoning_content: None,
                            tool_calls: vec![ToolCallDelta {
                                index: 0,
                                id: Some(id),
                                function_name,
                                function_arguments,
                            }],
                        },
                        finish_reason: None,
                        usage: None,
                    }
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => ChatStreamChunk {
                    delta: ChatStreamDelta {
                        content: None,
                        reasoning_content: Some(reasoning),
                        tool_calls: vec![],
                    },
                    finish_reason: None,
                    usage: None,
                },
                StreamedAssistantContent::Final(res) => {
                    let usage = res.token_usage().map(|u| Usage {
                        input_tokens: u.input_tokens as usize,
                        output_tokens: u.output_tokens as usize,
                        total_tokens: u.total_tokens as usize,
                    });
                    ChatStreamChunk {
                        delta: ChatStreamDelta::default(),
                        finish_reason: Some(FinishReason::Stop),
                        usage,
                    }
                }
                _ => ChatStreamChunk {
                    delta: ChatStreamDelta::default(),
                    finish_reason: None,
                    usage: None,
                },
            };
            Ok(chunk)
        }
        Err(e) => Err(ProviderError::Other(e.to_string())),
    });

    Box::pin(mapped)
}
