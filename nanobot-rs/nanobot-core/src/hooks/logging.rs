//! Logging hook — decouples observability from the agent loop.
//!
//! Replaces the hardcoded `log_response` and tool-level debug logging
//! with a hook that can be customized or replaced.

use tracing::{debug, info};

use super::{AgentHook, LlmResponseContext, ToolResultContext};

/// Default hook that logs LLM responses and tool results via `tracing`.
///
/// Registered automatically by [`AgentLoop::new()`].
pub struct LoggingHook;

impl LoggingHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AgentHook for LoggingHook {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn on_llm_response(&self, ctx: &mut LlmResponseContext) {
        if let Some(ref reasoning) = ctx.response.reasoning_content {
            if !reasoning.is_empty() {
                debug!("[Agent] Reasoning (iter {}): {}", ctx.iteration, reasoning);
            }
        }
        if let Some(ref content) = ctx.response.content {
            if !content.is_empty() {
                info!("[Agent] Response (iter {}): {}", ctx.iteration, content);
            }
        }
    }

    async fn on_tool_result(&self, ctx: &mut ToolResultContext) {
        let preview = if ctx.tool_result.len() > 500 {
            format!("{}... (truncated)", &ctx.tool_result[..500])
        } else {
            ctx.tool_result.clone()
        };
        debug!(
            "[Tool] {} -> {} ({}ms)",
            ctx.tool_name, preview, ctx.duration_ms
        );
    }
}
