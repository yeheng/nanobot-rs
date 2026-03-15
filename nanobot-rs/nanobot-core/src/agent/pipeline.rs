//! Agent pipeline - explicit sequential execution flow
//!
//! This provides a simplified pipeline alternative to AgentLoop:
//! 1. Load history (via context trait)
//! 2. Assemble prompt
//! 3. Core execution (AgentExecutor)
//! 4. Save result (via context trait)

use std::sync::Arc;

use crate::agent::context::AgentContext;
use crate::agent::executor_core::AgentExecutor;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::loop_::AgentConfig;
use crate::bus::events::SessionKey;
use crate::error::AgentError;
use crate::providers::{ChatMessage, LlmProvider};
use crate::session::SessionMessage;
use crate::tools::ToolRegistry;

/// Pipeline context - minimal dependencies
pub struct PipelineContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: AgentConfig,
    pub context: Arc<dyn AgentContext>,
    pub system_prompt: String,
    pub history_config: HistoryConfig,
}

/// Process a message through simplified pipeline
pub async fn process_message(
    ctx: &PipelineContext,
    session_key: &SessionKey,
    user_message: &str,
) -> Result<String, AgentError> {
    // 1. Load history
    let session = ctx.context.load_session(session_key).await;
    let history = session.messages;

    // 2. Save user message
    ctx.context
        .save_message(session_key, "user", user_message, None)
        .await;

    // 3. Process history
    let processed = process_history(history, &ctx.history_config);

    // 4. Load summary
    let summary = ctx.context.load_summary(&session_key.to_string()).await;

    // 5. Assemble prompt
    let messages = assemble_prompt(
        &ctx.system_prompt,
        summary.as_deref(),
        &processed.messages,
        user_message,
    );

    // 6. Core execution
    let executor = AgentExecutor::new(ctx.provider.clone(), ctx.tools.clone(), &ctx.config);
    let result = executor.execute(messages).await?;

    // 7. Save assistant message
    ctx.context
        .save_message(
            session_key,
            "assistant",
            &result.content,
            Some(result.tools_used),
        )
        .await;

    // 8. Background compression
    if !processed.evicted.is_empty() {
        ctx.context
            .compress_context(&session_key.to_string(), &processed.evicted);
    }

    Ok(result.content)
}

fn assemble_prompt(
    system_prompt: &str,
    summary: Option<&str>,
    history: &[SessionMessage],
    user_message: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    let mut sys = system_prompt.to_string();
    if let Some(sum) = summary {
        sys.push_str("\n\n# Previous Context Summary\n");
        sys.push_str(sum);
    }
    messages.push(ChatMessage::system(&sys));

    for msg in history {
        let role = msg.role.as_str();
        messages.push(match role {
            "user" => ChatMessage::user(&msg.content),
            "assistant" => ChatMessage::assistant(&msg.content),
            _ => ChatMessage::system(&msg.content),
        });
    }

    messages.push(ChatMessage::user(user_message));
    messages
}
