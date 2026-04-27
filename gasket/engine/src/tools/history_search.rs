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
    use gasket_storage::EventStore;

    /// Semantic history search tool using embedding-based recall.
    pub struct HistorySearchTool {
        searcher: Arc<RecallSearcher>,
        config: RecallConfig,
        event_store: Arc<EventStore>,
    }

    impl HistorySearchTool {
        /// Create a new history search tool.
        pub fn new(
            searcher: Arc<RecallSearcher>,
            config: RecallConfig,
            event_store: Arc<EventStore>,
        ) -> Self {
            Self {
                searcher,
                config,
                event_store,
            }
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

            let results = self
                .searcher
                .recall(&args.query, &config)
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Search failed: {}", e)))?;

            if results.is_empty() {
                return Ok("No semantically similar history found.".to_string());
            }

            // Load full event content for matched IDs.
            let ids: Vec<uuid::Uuid> = results
                .iter()
                .filter_map(|(id, _)| uuid::Uuid::parse_str(id).ok())
                .collect();

            let events = self
                .event_store
                .get_events_by_ids_global(&ids)
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Failed to load events: {}", e)))?;

            if events.is_empty() {
                return Ok("No matching events found in store.".to_string());
            }

            // Build a score lookup for display.
            let score_map: std::collections::HashMap<String, f32> = results.into_iter().collect();

            let mut lines = vec![format!(
                "Found {} semantically similar messages:",
                events.len()
            )];

            for event in &events {
                let score = score_map.get(&event.id.to_string()).copied().unwrap_or(0.0);
                let role = match event.event_type {
                    gasket_types::EventType::UserMessage => "user",
                    gasket_types::EventType::AssistantMessage => "assistant",
                    _ => "system",
                };
                let preview = if event.content.chars().count() > 400 {
                    format!("{}...", event.content.chars().take(400).collect::<String>())
                } else {
                    event.content.clone()
                };
                lines.push(format!(
                    "\n[{}] (score: {:.3}) {}:\n{}",
                    event.created_at, score, role, preview
                ));
            }

            Ok(lines.join(""))
        }
    }
}

#[cfg(feature = "embedding")]
pub use impl_::HistorySearchTool;
