//! Semantic history search tool using embedding-based recall.
//!
//! Gated by the `embedding` feature. Provides a `history_search` tool that
//! uses `RecallSearcher` to find semantically similar past conversation events.

#[cfg(feature = "embedding")]
mod impl_ {
    use std::sync::Arc;

    use async_trait::async_trait;
    use gasket_embedding::{RecallConfig, RecallSearcher};
    use serde::Deserialize;
    use serde_json::Value;
    use tracing::{debug, instrument};

    use super::super::{Tool, ToolContext, ToolError, ToolResult};

    /// Semantic history search tool using embedding-based recall.
    pub struct HistorySearchTool {
        searcher: Arc<RecallSearcher>,
        config: RecallConfig,
    }

    impl HistorySearchTool {
        /// Create a new history search tool.
        pub fn new(searcher: Arc<RecallSearcher>, config: RecallConfig) -> Self {
            Self { searcher, config }
        }
    }

    #[derive(Deserialize)]
    struct SearchArgs {
        /// The search query.
        query: String,
        /// Maximum number of results to return (overrides config default).
        top_k: Option<usize>,
    }

    #[async_trait]
    impl Tool for HistorySearchTool {
        fn name(&self) -> &str {
            "history_search"
        }

        fn description(&self) -> &str {
            "Search conversation history using semantic embedding similarity. \
             Returns the most relevant past messages based on meaning, not just keywords."
        }

        fn parameters(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query — describe what you're looking for in natural language"
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: from config)",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            })
        }

        #[instrument(name = "tool.history_search", skip_all)]
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        async fn execute(&self, params: Value, _ctx: &ToolContext) -> ToolResult {
            let args: SearchArgs = serde_json::from_value(params)
                .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

            debug!(
                "History search tool invoked: query={:?}, top_k={:?}",
                args.query, args.top_k
            );

            let mut config = self.config.clone();
            if let Some(top_k) = args.top_k {
                config.top_k = top_k.min(20);
            }

            let hits = self
                .searcher
                .recall(&args.query, &config)
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Search failed: {}", e)))?;

            if hits.is_empty() {
                return Ok("No semantically similar history found.".to_string().into());
            }

            let mut lines = vec![format!(
                "Found {} semantically similar messages:",
                hits.len()
            )];

            for hit in &hits {
                let preview = if hit.content.chars().count() > 400 {
                    format!("{}...", hit.content.chars().take(400).collect::<String>())
                } else {
                    hit.content.clone()
                };
                lines.push(format!(
                    "\n[{}] (score: {:.3}) {}:\n{}",
                    hit.created_at, hit.score, hit.role, preview
                ));
            }

            Ok(lines.join("").into())
        }
    }
}

#[cfg(feature = "embedding")]
pub use impl_::HistorySearchTool;
