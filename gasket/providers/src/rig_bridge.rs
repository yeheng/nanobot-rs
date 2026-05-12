//! Type conversion bridge between gasket provider types and rig-core types.

use rig::completion::{CompletionRequest, Message as RigMessage, ToolDefinition as RigToolDefinition};
use rig::message::{AssistantContent, ToolCall, ToolFunction};
use rig::OneOrMany;

use crate::{ChatMessage, ChatRequest, MessageRole};

/// Convert gasket ChatMessage to rig Message
pub fn to_rig_message(msg: ChatMessage) -> RigMessage {
    match msg.role {
        MessageRole::System => RigMessage::system(msg.content.unwrap_or_default()),
        MessageRole::User => RigMessage::user(msg.content.unwrap_or_default()),
        MessageRole::Assistant => {
            // Assistant messages with tool calls need special handling
            if let Some(tool_calls) = msg.tool_calls {
                let contents: Vec<AssistantContent> = tool_calls
                    .into_iter()
                    .map(|tc| {
                        AssistantContent::ToolCall(ToolCall::new(
                            tc.id,
                            ToolFunction {
                                name: tc.function.name,
                                arguments: tc.function.arguments,
                            },
                        ))
                    })
                    .collect();
                RigMessage::Assistant {
                    id: None,
                    content: OneOrMany::many(contents).unwrap_or_else(|_| {
                        OneOrMany::one(AssistantContent::text(msg.content.unwrap_or_default()))
                    }),
                }
            } else {
                RigMessage::assistant(msg.content.unwrap_or_default())
            }
        }
        MessageRole::Tool => {
            // Tool results map to user messages with ToolResult content
            RigMessage::user(msg.content.unwrap_or_default())
        }
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
        chat_history: OneOrMany::many(messages).unwrap_or_else(|_| OneOrMany::one(RigMessage::user(""))),
        documents: vec![],
        tools: request.tools.map(|tools| {
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