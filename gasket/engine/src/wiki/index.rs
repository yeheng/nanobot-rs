//! PageIndex: async wrapper over `PageSearchIndex` trait.

use std::sync::Arc;

use anyhow::Result;

use gasket_storage::wiki::{IndexPage, PageSearchIndex, SearchHit};

use super::page::{PageSummary, WikiPage};
use super::store::PageStore;

pub struct PageIndex {
    index: Arc<dyn PageSearchIndex>,
}

impl PageIndex {
    pub fn new(index: Arc<dyn PageSearchIndex>) -> Self {
        Self { index }
    }

    pub fn index(&self) -> &Arc<dyn PageSearchIndex> {
        &self.index
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<PageSummary>> {
        self.search_with_store(query, limit, None).await
    }

    pub async fn search_with_store(
        &self,
        query: &str,
        limit: usize,
        store: Option<&PageStore>,
    ) -> Result<Vec<PageSummary>> {
        let hits = self.index.search(query, limit).await?;

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
                        content_length: page.content.len() as u64,
                    }),
                    Err(e) => {
                        tracing::debug!("PageIndex: could not load page '{}': {}", hit.path, e);
                    }
                }
            }
            Ok(summaries)
        } else {
            Ok(vec![])
        }
    }

    pub async fn search_raw(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.index.search(query, limit).await
    }

    /// Upsert a wiki page into the search index.
    pub async fn upsert(&self, page: &WikiPage) -> Result<()> {
        self.index.upsert(&wiki_page_to_index(page)).await
    }

    /// Delete a page from the search index.
    pub async fn delete(&self, path: &str) -> Result<()> {
        self.index.delete(path).await
    }

    /// Rebuild the index from all pages in the store.
    pub async fn rebuild(&self, store: &PageStore) -> Result<usize> {
        let pages = store.list(Default::default()).await?;
        let mut full_pages = Vec::new();
        for summary in &pages {
            if let Ok(page) = store.read(&summary.path).await {
                full_pages.push(wiki_page_to_index(&page));
            }
        }
        self.index.rebuild(&full_pages).await
    }

    pub fn doc_count(&self) -> u64 {
        self.index.doc_count()
    }
}

/// Convert engine `WikiPage` to storage `IndexPage`.
fn wiki_page_to_index(page: &WikiPage) -> IndexPage {
    IndexPage {
        path: page.path.clone(),
        title: page.title.clone(),
        content: page.content.clone(),
        page_type: page.page_type.as_str().to_string(),
        category: page.category.clone(),
        tags: page.tags.clone(),
        confidence: page.confidence,
    }
}
