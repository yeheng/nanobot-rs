//! Wiki query pipeline — two-phase retrieval with Tantivy boost.
//!
//! Phase 1: Tantivy BM25 → candidate set (top-50, title boosted)
//! Phase 2: Budget-aware selection → load full pages from SQLite
//!
//! Reranker removed: title boost is handled in Tantivy query parsing.

pub mod tantivy_adapter;

pub use tantivy_adapter::{SearchHit, TantivyIndex};

use std::sync::Arc;

use anyhow::Result;

use super::page::{slugify, PageType, WikiPage};
use super::store::PageStore;

/// Token budget for query results (controls how much content to return).
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum tokens to return. Approximate: ~4 chars per token.
    pub max_tokens: usize,
}

impl TokenBudget {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// Default budget: 4000 tokens (~16000 chars).
    pub fn default_budget() -> Self {
        Self { max_tokens: 4000 }
    }

    /// Convert token count to approximate character count.
    pub fn chars_budget(&self) -> usize {
        self.max_tokens * 4
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::default_budget()
    }
}

/// Result of a wiki query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Loaded pages with content.
    pub pages: Vec<WikiPage>,
    /// Total candidates found before budget truncation.
    pub total_candidates: usize,
    /// Total tokens estimated for the returned pages.
    pub estimated_tokens: usize,
}

impl QueryResult {
    /// Format pages as a single context string for LLM injection.
    pub fn to_context_string(&self) -> String {
        let mut out = String::new();
        for page in &self.pages {
            if !out.is_empty() {
                out.push_str("\n---\n");
            }
            out.push_str(&format!(
                "## {} ({})\n\n{}",
                page.title, page.path, page.content
            ));
        }
        out
    }
}

/// Wiki query engine — two-phase retrieval over wiki pages.
pub struct WikiQueryEngine {
    tantivy: Arc<TantivyIndex>,
    store: Arc<PageStore>,
}

impl WikiQueryEngine {
    /// Create a new query engine.
    pub fn new(tantivy: Arc<TantivyIndex>, store: Arc<PageStore>) -> Self {
        Self {
            tantivy,
            store,
        }
    }

    /// Full two-phase query with budget-aware selection.
    ///
    /// Phase 1: Tantivy BM25 → top-50 candidates (title boosted)
    /// Phase 2: Budget selection → load pages from SQLite
    pub async fn query(&self, query: &str, budget: TokenBudget) -> Result<QueryResult> {
        // Phase 1: Candidate retrieval
        let candidates = self.tantivy.search(query, 50)?;
        let total_candidates = candidates.len();

        if candidates.is_empty() {
            return Ok(QueryResult {
                pages: vec![],
                total_candidates: 0,
                estimated_tokens: 0,
            });
        }

        // Phase 2: Budget-aware selection + load full pages
        let chars_budget = budget.chars_budget();
        let mut used_chars = 0usize;
        let mut pages = Vec::new();
        let mut estimated_tokens = 0usize;

        for hit in &candidates {
            match self.store.read(&hit.path).await {
                Ok(page) => {
                    let page_chars = page.content.len();
                    if used_chars + page_chars > chars_budget && !pages.is_empty() {
                        break; // Budget exhausted
                    }
                    used_chars += page_chars;
                    estimated_tokens += page_chars / 4;
                    pages.push(page);
                }
                Err(e) => {
                    tracing::debug!("WikiQuery: skip '{}': {}", hit.path, e);
                }
            }
        }

        Ok(QueryResult {
            pages,
            total_candidates,
            estimated_tokens,
        })
    }

    /// Simple BM25 search returning search hits (no reranking, no page loading).
    pub fn search_raw(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.tantivy.search(query, limit)
    }

    /// File a good answer back into the wiki as a new topic page.
    ///
    /// This is the "answer filing" feature: after a good Q&A exchange,
    /// the agent can save the knowledge for future retrieval.
    pub async fn file_answer(&self, question: &str, answer: &str) -> Result<String> {
        let path = format!("topics/{}", slugify(question));
        let page = WikiPage::new(
            path.clone(),
            question.to_string(),
            PageType::Topic,
            answer.to_string(),
        );
        self.store.write(&page).await?;

        // Also upsert into Tantivy index
        let full_page = self.store.read(&path).await?;
        self.tantivy.upsert(&full_page)?;

        tracing::info!("Filed answer as wiki page: '{}'", path);
        Ok(path)
    }

    /// Rebuild the Tantivy index from all pages in the store.
    pub async fn rebuild_index(&self) -> Result<usize> {
        let summaries = self.store.list(Default::default()).await?;
        let mut full_pages = Vec::new();
        for summary in &summaries {
            if let Ok(page) = self.store.read(&summary.path).await {
                full_pages.push(page);
            }
        }
        self.tantivy.rebuild(&full_pages)?;
        Ok(full_pages.len())
    }

    /// Get the underlying Tantivy index reference.
    pub fn tantivy(&self) -> &Arc<TantivyIndex> {
        &self.tantivy
    }

    /// Get the underlying page store reference.
    pub fn store(&self) -> &Arc<PageStore> {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_budget_default() {
        let budget = TokenBudget::default();
        assert_eq!(budget.max_tokens, 4000);
        assert_eq!(budget.chars_budget(), 16000);
    }

    #[test]
    fn test_query_result_to_context_string() {
        let result = QueryResult {
            pages: vec![WikiPage::new(
                "topics/rust".to_string(),
                "Rust".to_string(),
                PageType::Topic,
                "Rust is a systems language.".to_string(),
            )],
            total_candidates: 5,
            estimated_tokens: 6,
        };
        let ctx = result.to_context_string();
        assert!(ctx.contains("## Rust"));
        assert!(ctx.contains("topics/rust"));
        assert!(ctx.contains("systems language"));
    }

    #[test]
    fn test_token_budget_custom() {
        let budget = TokenBudget::new(1000);
        assert_eq!(budget.max_tokens, 1000);
        assert_eq!(budget.chars_budget(), 4000);
    }
}
