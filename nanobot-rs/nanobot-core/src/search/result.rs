//! Search result types.

use serde::{Deserialize, Serialize};

/// A single search result from Tantivy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Document ID (memory_id or message_id)
    pub id: String,

    /// Content/text of the document
    pub content: String,

    /// Relevance score (BM25)
    pub score: f32,

    /// Tags (for memory results)
    #[serde(default)]
    pub tags: Vec<String>,

    /// Source type (user/agent/system)
    #[serde(default)]
    pub source: Option<String>,

    /// Timestamp (ISO 8601)
    pub timestamp: Option<String>,

    /// Highlighted text snippet (optional)
    pub highlight: Option<HighlightedText>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// Highlighted text with matched portions marked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightedText {
    /// The highlighted text with markers
    pub text: String,

    /// The highlight marker used (e.g., "**" or "<mark>")
    #[serde(default = "default_marker")]
    pub marker: String,
}

fn default_marker() -> String {
    "**".to_string()
}

impl HighlightedText {
    /// Create a new highlighted text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            marker: default_marker(),
        }
    }

    /// Create with custom marker.
    pub fn with_marker(text: impl Into<String>, marker: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            marker: marker.into(),
        }
    }
}

impl SearchResult {
    /// Create a new search result.
    pub fn new(id: impl Into<String>, content: impl Into<String>, score: f32) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            score,
            tags: Vec::new(),
            source: None,
            timestamp: None,
            highlight: None,
            metadata: serde_json::Map::new(),
        }
    }

    /// Add tags to the result.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Add timestamp to the result.
    pub fn with_timestamp(mut self, ts: impl Into<String>) -> Self {
        self.timestamp = Some(ts.into());
        self
    }

    /// Add highlight to the result.
    pub fn with_highlight(mut self, highlight: HighlightedText) -> Self {
        self.highlight = Some(highlight);
        self
    }
}
