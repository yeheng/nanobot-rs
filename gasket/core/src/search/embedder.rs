//! Offline text embedding engine using fastembed (AllMiniLML6V2, 384-dim).
//!
//! Wraps `fastembed::TextEmbedding` in a `parking_lot::Mutex` because
//! the upstream `embed()` method requires `&mut self` (the tokenizer
//! mutates internal buffers during encoding).
//!
//! # Usage
//!
//! ```no_run
//! use gasket_core::search::TextEmbedder;
//!
//! let embedder = TextEmbedder::new().unwrap();
//! let vector = embedder.embed("hello world").unwrap();
//! assert_eq!(vector.len(), 384);
//! ```

use anyhow::Result;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use parking_lot::Mutex;
use std::path::PathBuf;
use tracing::info;

/// Embedding dimension for the AllMiniLML6V2 model.
pub const EMBEDDING_DIM: usize = 384;

/// Thread-safe text embedder backed by a local ONNX model.
///
/// The model weights (~20 MB) are downloaded once from HuggingFace and
/// cached locally.  Subsequent calls run entirely offline in < 10 ms
/// per sentence on modern hardware.
pub struct TextEmbedder {
    model: Mutex<TextEmbedding>,
}

impl TextEmbedder {
    /// Initialise with the default AllMiniLML6V2 model.
    ///
    /// **First run only:** downloads ~20 MB of model weights.
    pub fn new() -> Result<Self> {
        info!(
            "Initializing TextEmbedder (AllMiniLML6V2, {}d)...",
            EMBEDDING_DIM
        );
        let model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )?;
        info!("TextEmbedder ready");
        Ok(Self {
            model: Mutex::new(model),
        })
    }

    /// Initialise with a custom cache directory for the ONNX weights.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Result<Self> {
        info!(
            "Initializing TextEmbedder (cache: {:?}, {}d)...",
            cache_dir, EMBEDDING_DIM
        );
        let model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(true),
        )?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }

    /// Embed a single piece of text into a 384-dimensional vector.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut model = self.model.lock();
        let mut embeddings = model.embed(vec![text], None)?;
        embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("TextEmbedder returned empty result"))
    }

    /// Embed multiple texts in a single batch (more efficient than
    /// calling `embed()` in a loop).
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self.model.lock();
        let embeddings = model.embed(texts, None)?;
        Ok(embeddings)
    }

    /// Return the embedding dimension (always 384 for AllMiniLML6V2).
    pub const fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_dimension() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let vec = embedder.embed("hello world").expect("embed failed");
        assert_eq!(vec.len(), EMBEDDING_DIM);
    }

    #[test]
    fn test_embed_batch() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let texts = vec![
            "the weather is nice".to_string(),
            "write some rust code".to_string(),
        ];
        let vecs = embedder.embed_batch(&texts).expect("batch embed failed");
        assert_eq!(vecs.len(), 2);
        for v in &vecs {
            assert_eq!(v.len(), EMBEDDING_DIM);
        }
    }

    #[test]
    fn test_embed_empty_batch() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let vecs = embedder.embed_batch(&[]).expect("empty batch failed");
        assert!(vecs.is_empty());
    }
}
