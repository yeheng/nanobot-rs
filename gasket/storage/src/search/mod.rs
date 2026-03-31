//! Full-text search and semantic embedding types.
//!
//! Provides:
//! - Basic search types for memory search functionality
//! - Re-exports from `vector_math` for text embedding and vector math
//!
//! For advanced Tantivy-based full-text search, use the standalone `tantivy-mcp` server.

mod query;
mod result;

pub use query::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};
pub use result::{HighlightedText, SearchResult};

// Re-export always-available semantic types from our own vector_math module
pub use crate::vector_math::{cosine_similarity, top_k_similar};

// Re-export feature-gated types when local-embedding is enabled
#[cfg(feature = "local-embedding")]
pub use crate::embedder::{EmbeddingConfig, TextEmbedder, DEFAULT_CACHE_DIR, DEFAULT_MODEL};
#[cfg(feature = "local-embedding")]
pub use crate::vector_math::{bytes_to_embedding, embedding_to_bytes};
