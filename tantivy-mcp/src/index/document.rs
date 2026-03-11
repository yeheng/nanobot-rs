//! Document operations.

use chrono::{DateTime, Utc};
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
