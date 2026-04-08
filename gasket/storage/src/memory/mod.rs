//! Long-term memory system for explicit knowledge persistence.
//!
//! This module provides types and utilities for managing explicit long-term memory
//! stored as Markdown files in `~/.gasket/memory/*.md`. Unlike SQLite (which stores
//! machine-state like sessions and events), memory files store user-curated knowledge:
//! facts, preferences, decisions, and reference material.
//!
//! ## Architecture
//!
//! - **Scenario-based organization:** Memories are organized into directories by scenario
//!   (profile, active, knowledge, decisions, episodes, reference)
//! - **Frequency-based decay:** Memories are tagged with access frequency (hot, warm, cold)
//!   for automated lifecycle management
//! - **Token budget tracking:** Each memory tracks its token count for budget enforcement
//! - **Supersession:** Old versions can reference their replacements for audit trails
//! - **SQLite metadata:** File metadata is indexed in `memory_metadata` table for fast queries

mod embedder;
mod embedding_store;
mod frontmatter;
mod index;
mod lifecycle;
mod metadata_store;
mod path;
mod retrieval;
mod store;
mod types;
mod watcher;

pub use types::*;

// Re-export key path resolution functions
pub use path::{history_dir, list_memory_files, memory_base_dir, memory_file_path, scenario_dir};

// Re-export frontmatter parsing functions
pub use frontmatter::{
    estimate_tokens, extract_body, extract_frontmatter_raw, parse_frontmatter, parse_memory_file,
    serialize_memory_file,
};

// Re-export FileMemoryStore
pub use store::FileMemoryStore;

// Re-export index scanner (filesystem → MemoryIndexEntry)
pub use index::{FileIndexManager, MemoryIndexEntry};

// Re-export MetadataStore (SQLite-backed metadata queries)
pub use metadata_store::MetadataStore;

// Re-export EmbeddingStore
pub use embedding_store::{EmbeddingHit, EmbeddingStore};

// Re-export Embedder trait
pub use embedder::{Embedder, NoopEmbedder};

// Re-export RetrievalEngine
pub use retrieval::{RetrievalEngine, SearchResult};

// Re-export lifecycle types
pub use lifecycle::{AccessEntry, AccessLog, DecayReport, FlushReport, FrequencyManager};

// Re-export file watcher utilities and AutoIndexHandler
pub use watcher::{scenario_from_path, should_ignore, AutoIndexHandler, RefreshReport};
