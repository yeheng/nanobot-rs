//! Semantic text embedding and vector math for gasket AI assistant.
//!
//! This crate provides:
//! - Text embedding using local ONNX models (optional, feature-gated)
//! - Pure-Rust vector math for semantic similarity
//! - Top-K retrieval utilities
//!
//! ## Features
//!
//! - `local-embedding` - Enable local ONNX-based text embedding (requires ~20MB model download)
//!
//! ## Usage
//!
//! ```no_run
//! use gasket_semantic::{cosine_similarity, top_k_similar};
//!
//! let a = vec![1.0, 0.0, 0.0];
//! let b = vec![0.707, 0.707, 0.0];
//! let sim = cosine_similarity(&a, &b);
//! ```

#[cfg(feature = "local-embedding")]
mod embedder;
mod vector_math;

#[cfg(feature = "local-embedding")]
pub use embedder::{EmbeddingConfig, TextEmbedder, DEFAULT_CACHE_DIR, DEFAULT_MODEL};
pub use vector_math::{cosine_similarity, top_k_similar};

#[cfg(feature = "local-embedding")]
pub use vector_math::{bytes_to_embedding, embedding_to_bytes};
