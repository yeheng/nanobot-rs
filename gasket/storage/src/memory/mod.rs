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

mod embedding_store;
mod frontmatter;
mod index;
mod lifecycle;
mod path;
mod retrieval;
mod store;
mod types;

pub use types::*;

// Re-export key path resolution functions
pub use path::{
    history_dir, index_path, list_memory_files, memory_base_dir, memory_file_path, scenario_dir,
};

// Re-export frontmatter parsing functions
pub use frontmatter::{
    estimate_tokens, extract_body, parse_frontmatter, parse_memory_file, serialize_memory_file,
};

// Re-export FileMemoryStore
pub use store::FileMemoryStore;

// Re-export index manager
pub use index::{FileIndexManager, MemoryIndex, MemoryIndexEntry};

// Re-export EmbeddingStore
pub use embedding_store::{EmbeddingHit, EmbeddingStore};

// Re-export RetrievalEngine
pub use retrieval::{RetrievalEngine, SearchResult};

// Re-export lifecycle types
pub use lifecycle::{AccessLog, AccessEntry, DecayReport, FlushReport, FrequencyManager};
