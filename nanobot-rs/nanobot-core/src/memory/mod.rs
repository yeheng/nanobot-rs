//! Memory storage abstraction
//!
//! Provides a trait-based `MemoryStore` interface with multiple backends:
//! - `FileMemoryStore` — file-based storage (migrated from `agent/memory.rs`)
//! - `InMemoryStore` — in-memory storage (for testing)

mod store;

pub use store::{FileMemoryStore, InMemoryStore, MemoryEntry, MemoryQuery, MemoryStore};
