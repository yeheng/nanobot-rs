//! # Gasket Core
//!
//! This crate is a **convenience re-export layer** (facade) for the gasket framework.
//!
//! All implementation lives in specialized crates (engine, providers, storage, etc.).
//! This crate re-exports commonly used types for ergonomic imports:
//!
//! ```ignore
//! use gasket_core::agent::AgentLoop;  // instead of gasket_engine::agent::AgentLoop
//! ```

// Modules with local implementations or needed for path-based imports
pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod heartbeat;
pub mod memory;
pub mod providers;
pub mod token_tracker;
pub mod tools;
pub mod vault;

// Re-export everything from types crate (canonical source for shared types)
pub use gasket_types::*;

// Re-export commonly used config types at crate root for backward compatibility
pub use config::{Config, ConfigLoader, ModelRegistry, ProviderRegistry};

// Re-export history types (no module wrapper needed)
pub use gasket_history::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};

// Re-export semantic for local embedding support
pub use gasket_semantic as semantic;

// Re-export storage
pub use gasket_storage as storage;
