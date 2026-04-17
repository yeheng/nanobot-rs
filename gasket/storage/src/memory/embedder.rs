//! Embedder trait for generating text embeddings.
//!
//! This trait abstracts embedding generation so the storage crate
//! can remain decoupled from the actual embedder implementation.

/// Trait for text embedding generation.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Generate embedding for a single text.
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;

    /// Get the embedding dimension.
    fn dimension(&self) -> usize;

    /// Clone into a boxed trait object.
    fn clone_box(&self) -> Box<dyn Embedder>;
}

/// No-op embedder that returns zero vectors.
/// Used when embedding is disabled or unavailable.
pub struct NoopEmbedder {
    dimension: usize,
}

impl NoopEmbedder {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

#[async_trait::async_trait]
impl Embedder for NoopEmbedder {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0f32; self.dimension])
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn clone_box(&self) -> Box<dyn Embedder> {
        Box::new(Self {
            dimension: self.dimension,
        })
    }
}
