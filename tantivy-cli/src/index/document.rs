//! Document operations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Document for indexing.
#[derive(Debug, Clone)]
pub struct Document {
    /// Unique document ID.
    pub id: String,
    /// Field values.
    pub fields: Map<String, Value>,
    /// Expiration timestamp (for TTL).
    pub expires_at: Option<DateTime<Utc>>,
}

impl Document {
    /// Create a new document.
    pub fn new(id: impl Into<String>, fields: Map<String, Value>) -> Self {
        Self {
            id: id.into(),
            fields,
            expires_at: None,
        }
    }

    /// Set expiration time.
    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Get a field value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }
}

/// Batch document input for deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDocumentInput {
    /// Document ID.
    pub id: String,
    /// Field values.
    pub fields: Map<String, Value>,
    /// Optional TTL (e.g., "7d").
    #[serde(default)]
    pub ttl: Option<String>,
}

impl From<BatchDocumentInput> for Document {
    fn from(input: BatchDocumentInput) -> Self {
        Self::new(input.id, input.fields)
    }
}

/// Batch operation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    /// Total number of documents.
    pub total: usize,
    /// Number of successful operations.
    pub success: usize,
    /// Number of failed operations.
    pub failed: usize,
    /// List of failed document IDs and errors.
    pub errors: Vec<BatchError>,
}

/// Batch operation error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchError {
    /// Document ID that failed.
    pub id: String,
    /// Error message.
    pub error: String,
}

/// Document operations trait.
pub trait DocumentOperations {
    /// Add or update a document.
    fn add_document(&mut self, document: Document) -> crate::Result<()>;

    /// Delete a document by ID.
    fn delete_document(&mut self, id: &str) -> crate::Result<()>;

    /// Get a document by ID.
    fn get_document(&self, id: &str) -> crate::Result<Option<Document>>;

    /// Count documents in the index.
    fn document_count(&self) -> u64;
}
