//! Memory storage
//!
//! SQLite is used exclusively for machine-state (sessions, summaries, cron, kv).
//! Explicit long-term memory lives in `~/.gasket/memory/*.md` files (SSOT).
//!
//! The actual storage implementation is in the `gasket-storage` crate.
//! This module re-exports the public API for backward compatibility.

// Re-export from gasket-storage crate
pub use gasket_storage::{config_dir, CronJobRow, EventStore, SqliteStore, StoreError};
