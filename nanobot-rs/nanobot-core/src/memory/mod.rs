//! Memory storage abstraction
//!
//! Provides a trait-based `MemoryStore` interface backed by `SqliteStore`.

mod sqlite;
mod store;

pub use sqlite::SqliteStore;
pub use store::{MemoryEntry, MemoryMetadata, MemoryQuery, MemoryStore};
