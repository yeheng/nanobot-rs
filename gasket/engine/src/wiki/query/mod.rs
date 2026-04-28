//! Wiki query pipeline — two-phase retrieval with Tantivy boost.
//!
//! Phase 1: BM25 → candidate set (top-50, title boosted)
//! Phase 2: Budget-aware selection → load full pages from SQLite

use std::sync::Arc;

use anyhow::Result;

use gasket_storage::wiki::{PageSearchIndex, SearchHit};

use super::page::{slugify, PageType, WikiPage};
use super::store::PageStore;

/// Token budget for query results (controls how much content to return).
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub max_tokens: usize,
}

impl TokenBudget {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    pub fn default_budget() -> Self {
        Self { max_tokens: 4000 }
    }

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
    pub pages: Vec<WikiPage>,
    pub total_candidates: usize,
    pub estimated_tokens: usize,
}

impl QueryResult {
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
    search: Arc<dyn PageSearchIndex>,
    store: PageStore,
}

impl WikiQueryEngine {
    pub fn new(search: Arc<dyn PageSearchIndex>, store: PageStore) -> Self {
        Self { search, store }
    }

    /// Full two-phase query with budget-aware selection.
    pub async fn query(&self, query: &str, budget: TokenBudget) -> Result<QueryResult> {
        let candidates = self.search.search(query, 50).await?;
        let total_candidates = candidates.len();

        if candidates.is_empty() {
            return Ok(QueryResult {
                pages: vec![],
                total_candidates: 0,
                estimated_tokens: 0,
            });
        }

        let paths: Vec<String> = candidates.iter().map(|h| h.path.clone()).collect();
        let summaries = self.store.read_summaries(&paths).await?;

        let summary_by_path: std::collections::HashMap<&str, &super::page::PageSummary> =
            summaries.iter().map(|s| (s.path.as_str(), s)).collect();

        let chars_budget = budget.chars_budget();
        let mut used_chars = 0usize;
        let mut selected_paths = Vec::new();
        let mut estimated_tokens = 0usize;

        for hit in &candidates {
            let Some(summary) = summary_by_path.get(hit.path.as_str()) else {
                tracing::debug!("WikiQuery: summary not found for '{}'", hit.path);
                continue;
            };
            let page_chars = summary.content_length as usize;
            if used_chars + page_chars > chars_budget && !selected_paths.is_empty() {
                break;
            }
            used_chars += page_chars;
            estimated_tokens += page_chars / 4;
            selected_paths.push(hit.path.as_str());
        }

        let selected_paths_owned: Vec<String> =
            selected_paths.into_iter().map(|s| s.to_string()).collect();
        let pages = self.store.read_many(&selected_paths_owned).await?;

        Ok(QueryResult {
            pages,
            total_candidates,
            estimated_tokens,
        })
    }

    /// Simple BM25 search returning search hits.
    pub async fn search_raw(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.search.search(query, limit).await
    }

    /// File a good answer back into the wiki as a new topic page.
    pub async fn file_answer(&self, question: &str, answer: &str) -> Result<String> {
        let path = format!("topics/{}", slugify(question));
        let page = WikiPage::new(
            path.clone(),
            question.to_string(),
            PageType::Topic,
            answer.to_string(),
        );
        self.store.write(&page).await?;

        let full_page = self.store.read(&path).await?;
        self.search
            .upsert(&gasket_storage::wiki::IndexPage {
                path: full_page.path.clone(),
                title: full_page.title.clone(),
                content: full_page.content.clone(),
                page_type: full_page.page_type.as_str().to_string(),
                category: full_page.category.clone(),
                tags: full_page.tags.clone(),
                confidence: full_page.confidence,
            })
            .await?;

        tracing::info!("Filed answer as wiki page: '{}'", path);
        Ok(path)
    }

    /// Rebuild the Tantivy index from all pages in the store.
    pub async fn rebuild_index(&self) -> Result<usize> {
        let summaries = self.store.list(Default::default()).await?;
        let mut index_pages = Vec::new();
        for summary in &summaries {
            if let Ok(page) = self.store.read(&summary.path).await {
                index_pages.push(gasket_storage::wiki::IndexPage {
                    path: page.path.clone(),
                    title: page.title.clone(),
                    content: page.content.clone(),
                    page_type: page.page_type.as_str().to_string(),
                    category: page.category.clone(),
                    tags: page.tags.clone(),
                    confidence: page.confidence,
                });
            }
        }
        let count = index_pages.len();
        self.search.rebuild(&index_pages).await?;
        Ok(count)
    }

    pub fn search(&self) -> &Arc<dyn PageSearchIndex> {
        &self.search
    }

    pub fn store(&self) -> &PageStore {
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
