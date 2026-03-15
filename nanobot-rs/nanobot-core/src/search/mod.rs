//! Full-text search types.
//!
//! Provides basic search types for memory search functionality.
//! For advanced Tantivy-based full-text search, use the standalone `tantivy-mcp` server.

mod query;
mod result;

pub use query::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};
pub use result::{HighlightedText, SearchResult};
