//! Query types for Tantivy search.

use serde::{Deserialize, Serialize};

/// A search query for Tantivy indexes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Full-text search keywords
    pub text: Option<String>,

    /// Boolean query (AND/OR/NOT logic)
    pub boolean: Option<BooleanQuery>,

    /// Fuzzy query with typo tolerance
    pub fuzzy: Option<FuzzyQuery>,

    /// Tag filters (AND semantics)
    pub tags: Vec<String>,

    /// Date range filter
    pub date_range: Option<DateRange>,

    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,

    /// Offset for pagination
    #[serde(default)]
    pub offset: usize,

    /// Sort order
    #[serde(default)]
    pub sort: SortOrder,

    /// Filter by role (for history search)
    pub role: Option<String>,

    /// Filter by session key (for history search)
    pub session_key: Option<String>,
}

fn default_limit() -> usize {
    10
}

/// Boolean query with must/should/not logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BooleanQuery {
    /// Terms that MUST be present (AND)
    pub must: Vec<String>,

    /// Terms that SHOULD be present (OR, boosts score)
    pub should: Vec<String>,

    /// Terms that MUST NOT be present (NOT)
    pub not: Vec<String>,
}

/// Fuzzy query with edit distance tolerance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzyQuery {
    /// The text to search
    pub text: String,

    /// Maximum edit distance (default: 2)
    #[serde(default = "default_distance")]
    pub distance: u8,

    /// Whether to allow prefix matching
    #[serde(default)]
    pub prefix: bool,
}

fn default_distance() -> u8 {
    2
}

/// Date range filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    /// Start date (inclusive), ISO 8601 format
    pub from: Option<String>,

    /// End date (inclusive), ISO 8601 format
    pub to: Option<String>,
}

/// Sort order for search results.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Sort by BM25 relevance score (highest first)
    #[default]
    Relevance,

    /// Sort by date (most recent first)
    Date,
}

impl SearchQuery {
    /// Create a simple text search query.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// Add a limit to the query.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Add tags filter to the query.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}
