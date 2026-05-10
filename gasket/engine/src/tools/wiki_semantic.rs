//! Bridge adapters connecting gasket-embedding's VectorStore/EmbeddingProvider
//! to wiki's WikiVectorStore/WikiEmbeddingProvider traits.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use gasket_embedding::{EmbeddingProvider, VectorStore};
use crate::wiki::{WikiEmbeddingProvider, WikiVectorHit, WikiVectorStore};

/// Adapter: wraps an `EmbeddingProvider` as a `WikiEmbeddingProvider`.
pub struct WikiEmbeddingAdapter {
    inner: Arc<dyn EmbeddingProvider>,
}

impl WikiEmbeddingAdapter {
    pub fn new(inner: Arc<dyn EmbeddingProvider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl WikiEmbeddingProvider for WikiEmbeddingAdapter {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.inner.embed(text).await
    }
}

/// Adapter: wraps a `VectorStore` as a `WikiVectorStore`.
/// Uses "wiki:<path>" as the vector record ID convention.
pub struct WikiVectorAdapter {
    inner: Arc<dyn VectorStore>,
}

impl WikiVectorAdapter {
    pub fn new(inner: Arc<dyn VectorStore>) -> Self {
        Self { inner }
    }

    fn wiki_id(path: &str) -> String {
        format!("wiki:{}", path)
    }
}

#[async_trait]
impl WikiVectorStore for WikiVectorAdapter {
    async fn upsert(&self, id: &str, vector: Vec<f32>, _content: &str) -> anyhow::Result<()> {
        let record = gasket_embedding::VectorRecord {
            id: Self::wiki_id(id),
            vector,
            session_key: format!("wiki:{}", id),
            event_type: "wiki_page".to_string(),
            content_hash: id.to_string(),
        };
        self.inner.upsert(vec![record]).await
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
    ) -> anyhow::Result<Vec<WikiVectorHit>> {
        let results = self
            .inner
            .search(query, top_k, min_score, &HashSet::new())
            .await?;
        Ok(results
            .into_iter()
            .filter_map(|r| {
                // Strip "wiki:" prefix from ID.
                let path = r.id.strip_prefix("wiki:")?.to_string();
                Some(WikiVectorHit {
                    id: path,
                    score: r.score,
                })
            })
            .collect())
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.inner.delete(&[Self::wiki_id(id)]).await?;
        Ok(())
    }
}
