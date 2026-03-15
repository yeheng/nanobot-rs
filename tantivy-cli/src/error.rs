//! Error types for tantivy-cli.

use std::path::PathBuf;
use thiserror::Error;

/// Error types for tantivy-cli.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Document not found: {0}")]
    DocumentNotFound(String),

    #[error("Schema error: {0}")]
    SchemaError(String),

    #[error("Unknown field: {0}")]
    UnknownField(String),

    #[error("Invalid field value: {0}")]
    InvalidFieldValue(String),

    #[error("Index error: {0}")]
    IndexError(#[from] tantivy::TantivyError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Query error: {0}")]
    QueryError(String),

    #[error("Writer not initialized")]
    WriterNotInitialized,

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Path error: {0}")]
    PathError(PathBuf, String),

    #[error("Lock error: {0}")]
    LockError(String),
}

impl From<tantivy::query::QueryParserError> for Error {
    fn from(e: tantivy::query::QueryParserError) -> Self {
        Error::QueryError(e.to_string())
    }
}

/// Result type alias for tantivy-cli.
pub type Result<T> = std::result::Result<T, Error>;
