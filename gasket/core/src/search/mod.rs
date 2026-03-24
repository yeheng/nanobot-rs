//! Full-text search and semantic embedding types.
//!
//! Provides:
//! - Basic search types for memory search functionality
//! - Offline text embedding engine (fastembed + ONNX)
//! - Pure-Rust vector math for semantic similarity
//!
//! For advanced Tantivy-based full-text search, use the standalone `tantivy-mcp` server.

mod embedder;
mod query;
mod result;
mod vector_math;

pub use embedder::{TextEmbedder, EMBEDDING_DIM};
pub use query::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};
pub use result::{HighlightedText, SearchResult};
pub use vector_math::{bytes_to_embedding, cosine_similarity, embedding_to_bytes, top_k_similar};
