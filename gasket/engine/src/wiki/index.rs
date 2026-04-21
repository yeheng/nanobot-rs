//! PageIndex: Tantivy-backed full-text search over wiki pages.
//!
//! Wraps `TantivyIndex` for BM25 search. The index lives at
//! `~/.gasket/wiki/.tantivy/` and is kept in sync with SQLite
//! via upsert on every page write.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use super::page::PageSummary;
use super::query::tantivy_adapter::{SearchHit, TantivyIndex};
use super::store::PageStore;

/// PageIndex: search over wiki pages using Tantivy BM25.
pub struct PageIndex {
    tantivy: Arc<TantivyIndex>,
}

impl PageIndex {
    /// Create a new PageIndex with a Tantivy index at the given directory.
    pub fn open(index_dir: PathBuf) -> Result<Self> {
        let tantivy = TantivyIndex::open(index_dir)?;
        Ok(Self {
            tantivy: Arc::new(tantivy),
        })
    }

    /// Create with an existing TantivyIndex (for sharing with WikiQueryEngine).
    pub fn from_tantivy(tantivy: Arc<TantivyIndex>) -> Self {
        Self { tantivy }
    }

    /// Get a reference to the underlying Tantivy index.
    pub fn tantivy(&self) -> &Arc<TantivyIndex> {
        &self.tantivy
    }

    /// Search wiki pages by query string. Returns ranked results via BM25.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<PageSummary>> {
        // Use a default filter to get all types from store
        self.search_with_store(query, limit, None).await
    }

    /// Search and return PageSummary by loading from the store.
    /// Falls back to SearchHit-only results if no store is provided.
    pub async fn search_with_store(
        &self,
        query: &str,
        limit: usize,
        store: Option<&PageStore>,
    ) -> Result<Vec<PageSummary>> {
        let hits = self.tantivy.search(query, limit)?;

        if let Some(store) = store {
            let mut summaries = Vec::new();
            for hit in hits {
                match store.read(&hit.path).await {
                    Ok(page) => summaries.push(PageSummary {
                        path: page.path,
                        title: page.title,
                        page_type: page.page_type,
                        category: page.category,
                        tags: page.tags,
                        updated: page.updated,
                        confidence: page.confidence,
                        frequency: page.frequency,
                        access_count: page.access_count,
                        last_accessed: page.last_accessed,
                    }),
                    Err(e) => {
                        tracing::debug!("PageIndex: could not load page '{}': {}", hit.path, e);
                    }
                }
            }
            Ok(summaries)
        } else {
            // No store available — return empty (can't build full PageSummary from index alone)
            tracing::debug!(
                "PageIndex: search returned {} hits but no store to load summaries",
                hits.len()
            );
            Ok(vec![])
        }
    }

    /// Raw BM25 search returning SearchHit (path + score + title).
    pub fn search_raw(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.tantivy.search(query, limit)
    }

    /// Upsert a page into the search index.
    pub fn upsert(&self, page: &super::page::WikiPage) -> Result<()> {
        self.tantivy.upsert(page)
    }

    /// Delete a page from the search index.
    pub fn delete(&self, path: &str) -> Result<()> {
        self.tantivy.delete(path)
    }

    /// Rebuild the index from all pages in the store.
    pub async fn rebuild(&self, store: &PageStore) -> Result<usize> {
        let pages = store.list(Default::default()).await?;
        let mut full_pages = Vec::new();
        for summary in &pages {
            if let Ok(page) = store.read(&summary.path).await {
                full_pages.push(page);
            }
        }
        self.tantivy.rebuild(&full_pages)?;
        Ok(full_pages.len())
    }

    /// Number of documents in the index.
    pub fn doc_count(&self) -> u64 {
        self.tantivy.doc_count()
    }
}
