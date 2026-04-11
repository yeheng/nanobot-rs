//! HistoryRecallHook - Hook for semantic history recall
//!
//! This hook performs semantic similarity search to recall relevant
//! historical messages from the embedding store and injects them
//! into the context before prompt assembly.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use super::{HookAction, HookPoint, MutableContext, PipelineHook, ReadonlyContext};
use crate::error::AgentError;
use crate::session::context::AgentContext;
use gasket_providers::ChatMessage;
use gasket_storage::TextEmbedder;

/// Hook for semantic history recall.
///
/// This hook executes at `AfterHistory` point and uses semantic similarity
/// to find relevant past messages from the embedding store, then injects
/// them into the context as a system-like message.
///
/// # How it Works
///
/// 1. Embeds the current user query using `TextEmbedder`
/// 2. Searches the embedding store for top-K similar messages
/// 3. Injects recalled messages as a context message
///
/// # Configuration
///
/// - `embedder`: The text embedder for creating query vectors
/// - `k`: Number of similar messages to recall (0 = disabled)
/// - `context`: Agent context for accessing the embedding store
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use gasket_core::hooks::HistoryRecallHook;
/// use gasket_core::search::TextEmbedder;
/// use gasket_core::agent::context::AgentContext;
///
/// // Create embedder
/// let embedder = Arc::new(TextEmbedder::new()?);
///
/// // Create hook with recall limit
/// let hook = HistoryRecallHook::new(embedder, 5, context);
/// ```
pub struct HistoryRecallHook {
    embedder: Arc<TextEmbedder>,
    k: usize,
    context: AgentContext,
}

impl HistoryRecallHook {
    /// Create a new HistoryRecallHook.
    ///
    /// # Arguments
    ///
    /// * `embedder` - Text embedder for creating query vectors
    /// * `k` - Number of similar messages to recall (0 = disabled)
    /// * `context` - Agent context for accessing embedding store
    pub fn new(embedder: Arc<TextEmbedder>, k: usize, context: AgentContext) -> Self {
        Self {
            embedder,
            k,
            context,
        }
    }
}

#[async_trait]
impl PipelineHook for HistoryRecallHook {
    fn name(&self) -> &str {
        "history_recall"
    }

    fn point(&self) -> HookPoint {
        HookPoint::AfterHistory
    }

    async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
        if self.k == 0 {
            return Ok(HookAction::Continue);
        }

        let query = ctx.user_input.unwrap_or("");

        match self.embedder.embed(query) {
            Ok(query_vec) => {
                match self
                    .context
                    .recall_history(ctx.session_key, &query_vec, self.k)
                    .await
                {
                    Ok(recalled) if !recalled.is_empty() => {
                        debug!("[HistoryRecall] Recalled {} messages", recalled.len());

                        let recall_msg =
                            format!("# Relevant Historical Context\n{}", recalled.join("\n"));
                        ctx.messages.push(ChatMessage::assistant(recall_msg));
                    }
                    Ok(_) => {
                        debug!("[HistoryRecall] No relevant history found");
                    }
                    Err(e) => {
                        debug!("[HistoryRecall] Recall failed: {}", e);
                    }
                }
                Ok(HookAction::Continue)
            }
            Err(e) => {
                debug!("[HistoryRecall] Failed to embed query: {}", e);
                Ok(HookAction::Continue)
            }
        }
    }

    async fn run_parallel(&self, _ctx: &ReadonlyContext<'_>) -> Result<HookAction, AgentError> {
        // HistoryRecallHook is Sequential only
        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_recall_hook_point() {
        // This is a compile-time test to verify the trait is implemented correctly
        // We can't fully test without a real TextEmbedder and AgentContext
        // but we can verify the hook point is correct
        let point = HookPoint::AfterHistory;
        assert_eq!(
            point.default_strategy(),
            crate::hooks::ExecutionStrategy::Sequential
        );
    }
}
