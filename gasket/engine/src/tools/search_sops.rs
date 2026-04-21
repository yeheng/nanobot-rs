//! Tool: search_sops — find relevant SOPs by query string.
//!
//! Queries the wiki index and filters for SOP pages only.
//! Available to the LLM during any turn for self-discovered knowledge retrieval.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{PageIndex, PageType};

/// Search the wiki for SOP pages relevant to the given query.
///
/// Returns up to `limit` hits, filtered to `PageType::Sop` only.
pub async fn search_sops(
    page_index: &PageIndex,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<crate::wiki::PageSummary>> {
    // Over-fetch to account for filtering, then truncate
    let fetch_limit = limit * 3;
    let results = page_index.search(query, fetch_limit).await?;

    let sops: Vec<_> = results
        .into_iter()
        .filter(|p| p.page_type == PageType::Sop)
        .take(limit)
        .collect();

    Ok(sops)
}

// ── SearchSopsTool ───────────────────────────────────────────────

/// Tool wrapper for searching SOP pages in the wiki.
pub struct SearchSopsTool {
    page_index: Arc<PageIndex>,
}

impl SearchSopsTool {
    pub fn new(page_index: Arc<PageIndex>) -> Self {
        Self { page_index }
    }
}

#[derive(Deserialize)]
struct SearchSopsArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

#[async_trait]
impl Tool for SearchSopsTool {
    fn name(&self) -> &str {
        "search_sops"
    }

    fn description(&self) -> &str {
        "Search the wiki for Standard Operating Procedure (SOP) pages relevant to a query. \
         Returns up to `limit` matching SOPs with title, path, and relevance score. \
         Use this when the agent needs to follow established procedures or best practices."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("query", "string", true, "Search query text"),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results (default: 5)",
            ),
        ])
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Option<Box<dyn Tool>> {
        Some(Box::new(Self {
            page_index: self.page_index.clone(),
        }))
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: SearchSopsArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let query = parsed.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidArguments(
                "query must not be empty".to_string(),
            ));
        }

        let sops = search_sops(&self.page_index, query, parsed.limit)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("SOP search failed: {}", e)))?;

        if sops.is_empty() {
            return Ok(format!(
                "No SOP pages found matching '{}'. Consider creating one.",
                query
            ));
        }

        let mut output = format!(
            "Found {} SOP page{} matching '{}':\n\n",
            sops.len(),
            if sops.len() == 1 { "" } else { "s" },
            query
        );

        for summary in &sops {
            let tags_display = if summary.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", summary.tags.join(", "))
            };
            output.push_str(&format!(
                "━━━ {} ━━━\n  Path: {}\n  Updated: {}\n  Relevance: {:.1}{}\n\n",
                summary.title,
                summary.path,
                summary.updated.format("%Y-%m-%d %H:%M UTC"),
                summary.confidence,
                tags_display
            ));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_search_sops_module_compiles() {
        // search_sops is a thin async wrapper — integration tests cover
        // actual search via wiki_integration tests.
        assert!(true);
    }
}
