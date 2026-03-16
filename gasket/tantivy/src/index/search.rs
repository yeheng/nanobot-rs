//! Search types and operations.

use serde::{Deserialize, Serialize};

/// Sort order for search results.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Sort by BM25 relevance score (highest first).
    #[default]
    Relevance,
    /// Sort by field value.
    Field,
}

/// Search query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Full-text search keywords.
    pub text: Option<String>,
    /// Field filters.
    #[serde(default)]
    pub filters: Vec<FieldFilter>,
    /// Maximum number of results.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: usize,
    /// Sort configuration.
    #[serde(default)]
    pub sort: Option<SortConfig>,
    /// Highlight configuration.
    #[serde(default)]
    pub highlight: Option<HighlightConfig>,
}

fn default_limit() -> usize {
    10
}

/// Highlight configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightConfig {
    /// Fields to highlight (empty = all text fields).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Maximum snippet length in characters.
    #[serde(default = "default_max_snippet_length")]
    pub max_length: usize,
    /// Number of snippets to return per field.
    #[serde(default = "default_num_snippets")]
    pub num_snippets: usize,
    /// HTML tag for highlighting matches (e.g., "mark" produces <mark>...</mark>).
    #[serde(default = "default_highlight_tag")]
    pub highlight_tag: String,
}

fn default_max_snippet_length() -> usize {
    150
}

fn default_num_snippets() -> usize {
    1
}

fn default_highlight_tag() -> String {
    "mark".to_string()
}

impl Default for HighlightConfig {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            max_length: default_max_snippet_length(),
            num_snippets: default_num_snippets(),
            highlight_tag: default_highlight_tag(),
        }
    }
}

/// Field filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldFilter {
    /// Field name.
    pub field: String,
    /// Comparison operator.
    pub op: FilterOp,
    /// Value to compare against.
    pub value: serde_json::Value,
}

/// Filter operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Contains (for string arrays).
    Contains,
}

/// Sort configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortConfig {
    /// Field to sort by.
    pub field: String,
    /// Sort order.
    #[serde(default)]
    pub order: SortDirection,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

/// Search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Document ID.
    pub id: String,
    /// Document fields.
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// Relevance score.
    #[serde(default)]
    pub score: f32,
    /// Highlighted snippets per field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<serde_json::Map<String, serde_json::Value>>,
    /// Legacy highlight field for backward compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
}

impl SearchQuery {
    /// Create a simple text search query.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// Add a limit.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Add a filter.
    pub fn with_filter(
        mut self,
        field: impl Into<String>,
        op: FilterOp,
        value: serde_json::Value,
    ) -> Self {
        self.filters.push(FieldFilter {
            field: field.into(),
            op,
            value,
        });
        self
    }

    /// Add highlighting configuration.
    pub fn with_highlight(mut self, fields: Vec<String>, max_length: usize) -> Self {
        self.highlight = Some(HighlightConfig {
            fields,
            max_length,
            ..Default::default()
        });
        self
    }
}
