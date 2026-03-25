//! Full-text search and semantic embedding types.
//!
//! Provides:
//! - Basic search types for memory search functionality
//! - Re-exports from `gasket-semantic` for text embedding and vector math
//!
//! For advanced Tantivy-based full-text search, use the standalone `tantivy-mcp` server.

mod query;
mod result;

pub use query::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};
pub use result::{HighlightedText, SearchResult};

// Re-export semantic types from gasket-semantic crate
pub use gasket_semantic::{
    bytes_to_embedding, cosine_similarity, embedding_to_bytes, top_k_similar, TextEmbedder,
};
