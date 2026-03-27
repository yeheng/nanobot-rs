//! Agent pipeline - explicit sequential execution flow
//!
//! This provides a simplified pipeline alternative to AgentLoop:
//! 1. Load history (via context enum)
//! 2. Assemble prompt
//! 3. Core execution (AgentExecutor)
//! 4. Save result (via context enum)

use std::sync::Arc;

use crate::agent::context::AgentContext;
use crate::agent::executor_core::AgentExecutor;
use crate::agent::loop_::AgentConfig;
use crate::tools::ToolRegistry;
use gasket_core::error::AgentError;
use gasket_providers::{ChatMessage, LlmProvider};
use gasket_types::SessionKey;
use gasket_types::{EventMetadata, EventType, SessionEvent};

/// Pipeline context - minimal dependencies
pub struct PipelineContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: AgentConfig,
    pub context: AgentContext,
    pub system_prompt: String,
}

/// Process a message through simplified pipeline
pub async fn process_message(
    ctx: &PipelineContext,
    session_key: &SessionKey,
    user_message: &str,
) -> Result<String, AgentError> {
    let session_key_str = session_key.to_string();

    // 1. Load history
    let history_events = ctx.context.get_history(&session_key_str, None).await;

    // 2. Save user event
    let user_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: session_key_str.clone(),
        parent_id: None,
        event_type: EventType::UserMessage,
        content: user_message.to_string(),
        embedding: None,
        metadata: EventMetadata::default(),
        created_at: chrono::Utc::now(),
    };
    ctx.context.save_event(user_event).await?;

    // 3. Filter to user/assistant messages for prompt assembly
    let history_for_prompt: Vec<SessionEvent> = history_events
        .into_iter()
        .filter(|e| {
            matches!(
                e.event_type,
                EventType::UserMessage | EventType::AssistantMessage
            )
        })
        .collect();

    // 4. Assemble prompt
    let messages = assemble_prompt(&ctx.system_prompt, &history_for_prompt, user_message);

    // 5. Core execution
    let executor = AgentExecutor::new(ctx.provider.clone(), ctx.tools.clone(), &ctx.config);
    let result = executor.execute(messages).await?;

    // 6. Save assistant event
    let assistant_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: session_key_str.clone(),
        parent_id: None,
        event_type: EventType::AssistantMessage,
        content: result.content.clone(),
        embedding: None,
        metadata: EventMetadata {
            tools_used: result.tools_used.clone(),
            ..Default::default()
        },
        created_at: chrono::Utc::now(),
    };
    ctx.context.save_event(assistant_event).await?;

    Ok(result.content)
}

fn assemble_prompt(
    system_prompt: &str,
    history: &[SessionEvent],
    user_message: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    messages.push(ChatMessage::system(system_prompt));

    for event in history {
        match event.event_type {
            EventType::UserMessage => {
                messages.push(ChatMessage::user(&event.content));
            }
            EventType::AssistantMessage => {
                messages.push(ChatMessage::assistant(&event.content));
            }
            _ => {}
        }
    }

    messages.push(ChatMessage::user(user_message));
    messages
}
