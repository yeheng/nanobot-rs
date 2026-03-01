//! Summarization hook — decouples context compression from the agent loop.
//!
//! Wraps [`SummarizationService`] and implements `on_context_prepare` to
//! compress evicted messages into a summary injected into the prompt.

use crate::agent::summarization::SummarizationService;

use super::{AgentHook, ContextPrepareContext};

/// Hook that compresses evicted history messages via LLM summarization.
///
/// Registered automatically by [`AgentLoop::new()`].
pub struct SummarizationHook {
    service: SummarizationService,
}

impl SummarizationHook {
    /// Create a new summarization hook.
    pub fn new(service: SummarizationService) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl AgentHook for SummarizationHook {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn on_context_prepare(&self, ctx: &mut ContextPrepareContext) {
        // Only generate summary if no other hook has already provided one
        if ctx.summary.is_some() {
            return;
        }

        if !ctx.evicted_messages.is_empty() {
            // Evicted messages exist — generate/update summary
            let existing_summary = self.service.load_summary(&ctx.session_key).await;
            match self
                .service
                .summarize(&ctx.session_key, &ctx.evicted_messages, &existing_summary)
                .await
            {
                Ok(new_summary) => {
                    ctx.summary = Some(new_summary);
                }
                Err(e) => {
                    tracing::warn!(
                        "Summarization failed, using existing summary as fallback: {}",
                        e
                    );
                    ctx.summary = existing_summary;
                }
            }
        } else {
            // No evictions — just load any existing summary
            ctx.summary = self.service.load_summary(&ctx.session_key).await;
        }
    }
}
