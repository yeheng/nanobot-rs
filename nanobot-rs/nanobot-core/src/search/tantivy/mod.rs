//! Tantivy search index implementations.

mod history_index;
mod memory_index;

pub use history_index::HistoryIndex;
pub use memory_index::MemoryIndex;

/// Error type for Tantivy operations.
#[derive(Debug, thiserror::Error)]
pub enum TantivyError {
    #[error("Failed to create index directory: {0}")]
    DirectoryError(#[from] std::io::Error),

    #[error("Failed to open index: {0}")]
    OpenError(String),

    #[error("Index operation failed: {0}")]
    OperationError(String),

    #[error("Document not found: {0}")]
    NotFound(String),
}

impl From<tantivy::TantivyError> for TantivyError {
    fn from(e: tantivy::TantivyError) -> Self {
        TantivyError::OpenError(e.to_string())
    }
}
