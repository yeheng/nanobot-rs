//! PageIndex: Tantivy-backed full-text search over wiki pages.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use super::page::{PageSummary, WikiPage};
use super::query::tantivy_adapter::{SearchHit, TantivyIndex};
use super::store::PageStore;

pub struct PageIndex {
    tantivy: Arc<TantivyIndex>,
}

impl PageIndex {
    pub fn open(index_dir: PathBuf) -> Result<Self> {
        let tantivy = TantivyIndex::open(index_dir)?;
        Ok(Self {
            tantivy: Arc::new(tantivy),
        })
    }

    pub fn from_tantivy(tantivy: Arc<TantivyIndex>) -> Self {
        Self { tantivy }
    }

    pub fn tantivy(&self) -> &Arc<TantivyIndex> {
        &self.tantivy
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
        let hits = self.tantivy.search_async(query.to_string(), limit).await?;

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
        self.tantivy.search_async(query.to_string(), limit).await
    }

    /// Upsert a page into the search index using spawn_blocking.
    pub async fn upsert(&self, page: &WikiPage) -> Result<()> {
        let tantivy = self.tantivy.clone();
        let page = page.clone();
        tokio::task::spawn_blocking(move || tantivy.upsert(&page))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Delete a page from the search index using spawn_blocking.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let tantivy = self.tantivy.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || tantivy.delete(&path))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Rebuild the index from all pages in the store using spawn_blocking.
    pub async fn rebuild(&self, store: &PageStore) -> Result<usize> {
        let pages = store.list(Default::default()).await?;
        let mut full_pages = Vec::new();
        for summary in &pages {
            if let Ok(page) = store.read(&summary.path).await {
                full_pages.push(page);
            }
        }
        let tantivy = self.tantivy.clone();
        let pages_for_blocking = full_pages.clone();
        tokio::task::spawn_blocking(move || tantivy.rebuild(&pages_for_blocking))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    pub fn doc_count(&self) -> u64 {
        self.tantivy.doc_count()
    }
}
