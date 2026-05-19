//! Rig-based embedding adapter.

use crate::EmbeddingProvider;
use anyhow::{anyhow, Result};
use rig::embeddings::EmbeddingModel;

/// Adapter that wraps rig's `EmbeddingModel` to implement gasket's `EmbeddingProvider` trait.
pub struct RigEmbeddingAdapter<M: EmbeddingModel> {
    model: M,
    dim: Option<usize>,
}

impl<M: EmbeddingModel> RigEmbeddingAdapter<M> {
    /// Create a new adapter wrapping the given rig embedding model.
    pub fn new(model: M) -> Self {
        Self { model, dim: None }
    }

    /// Create a new adapter with an explicit dimension override.
    /// The configured `dim` takes precedence over `model.ndims()`.
    pub fn new_with_dim(model: M, dim: usize) -> Self {
        Self {
            model,
            dim: Some(dim),
        }
    }
}

#[async_trait::async_trait]
impl<M: EmbeddingModel + Send + Sync> EmbeddingProvider for RigEmbeddingAdapter<M> {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embedding = self
            .model
            .embed_text(text)
            .await
            .map_err(|e| anyhow!("rig embedding error: {}", e))?;
        Ok(embedding.vec.into_iter().map(|v| v as f32).collect())
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let embeddings = self
            .model
            .embed_texts(texts)
            .await
            .map_err(|e| anyhow!("rig embedding error: {}", e))?;
        Ok(embeddings
            .into_iter()
            .map(|e| e.vec.into_iter().map(|v| v as f32).collect())
            .collect())
    }

    fn dim(&self) -> usize {
        self.dim.unwrap_or_else(|| self.model.ndims())
    }
}
