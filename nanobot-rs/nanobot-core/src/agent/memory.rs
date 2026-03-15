//! Memory store for session management
//!
//! This module wraps `SqliteStore` to provide access to the underlying
//! database for session persistence, summaries, and cron jobs.
//!
//! **Note:** Explicit long-term memory (facts, preferences, decisions)
//! lives exclusively in `~/.nanobot/memory/*.md` files (SSOT).
//! SQLite is only used for machine-state.

use crate::memory::SqliteStore;
use sqlx::SqlitePool;

/// Memory store — thin wrapper over `SqliteStore` for machine-state.
///
/// Provides access to the underlying `SqliteStore` for session management,
/// summaries, and cron job persistence. Does **not** store explicit
/// long-term memories (those live in Markdown files).
pub struct MemoryStore {
    store: SqliteStore,
}

impl MemoryStore {
    /// Create a new memory store.
    ///
    /// Opens the default `SqliteStore` at `~/.nanobot/nanobot.db`.
    pub async fn new() -> Self {
        let store = SqliteStore::new()
            .await
            .expect("Failed to open SqliteStore");

        Self { store }
    }

    /// Create a memory store with a specific `SqliteStore` instance.
    pub fn with_store(store: SqliteStore) -> Self {
        Self { store }
    }

    /// Get a reference to the underlying `SqliteStore`.
    pub fn sqlite_store(&self) -> &SqliteStore {
        &self.store
    }

    /// Get a clone of the underlying SQLite pool.
    ///
    /// Useful for sharing the pool with other subsystems (e.g., pipeline).
    pub fn pool(&self) -> SqlitePool {
        self.store.pool()
    }
}
