//! Full-text search module using Tantivy.
//!
//! Provides high-performance search capabilities for:
//! - Memory files (`~/.nanobot/memory/*.md`)
//! - Session history (SQLite `session_messages` table)

pub mod query;
pub mod result;
pub mod tantivy;

pub use query::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};
pub use result::{HighlightedText, SearchResult};
