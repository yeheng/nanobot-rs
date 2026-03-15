//! Index management module.

mod document;
mod lock;
mod manager;
mod schema;
mod search;

pub use document::{BatchDocumentInput, BatchError, BatchResult, Document, DocumentOperations};
pub use lock::IndexLock;
pub use manager::IndexManager;
pub use schema::{FieldDef, FieldType, IndexConfig, IndexSchema};
pub use search::{SearchQuery, SearchResult, SortOrder};
