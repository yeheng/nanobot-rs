//! Memory store for session management
//!
//! This module wraps `SqliteStore` to provide access to the underlying
//! database for session persistence, summaries, and cron jobs.
//!
//! **Note:** Explicit long-term memory (facts, preferences, decisions)
//! lives exclusively in `~/.gasket/memory/*.md` files (SSOT).
//! SQLite is only used for machine-state.

use gasket_storage::SqlitePool;
use gasket_storage::SqliteStore;

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
    /// Opens the default `SqliteStore` at `~/.gasket/gasket.db`.
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

// ─────────────────────────────────────────────────────────────────────────────
// MemoryProvider trait (from original provider.rs)
// ─────────────────────────────────────────────────────────────────────────────

use anyhow::Result;
use async_trait::async_trait;
use gasket_storage::memory::{MemoryHit, MemoryQuery, Scenario};
use gasket_types::session_event::SessionEvent;

use crate::agent::memory::manager::MemoryContext;

/// MemoryProvider trait — memory system query and mutation interface.
///
/// Extracted from MemoryManager to allow:
/// - HistoryCoordinator to depend on trait, not concrete type
/// - Testing with mock implementations
/// - Future alternative memory backends
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Three-phase loading (bootstrap/scenario/on-demand).
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext>;

    /// Semantic search across memories.
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>>;

    /// Extract knowledge from event.
    async fn update_from_event(&self, _event: &SessionEvent) -> Result<()> {
        // Default: no-op.
        Ok(())
    }

    /// Create a new memory file and sync metadata to SQLite (write-through).
    async fn create_memory(
        &self,
        scenario: Scenario,
        filename: &str,
        title: &str,
        tags: &[String],
        frequency: gasket_storage::memory::Frequency,
        content: &str,
    ) -> Result<()>;

    /// Update an existing memory file and sync metadata to SQLite (write-through).
    async fn update_memory(&self, scenario: Scenario, filename: &str, content: &str) -> Result<()>;

    /// Delete a memory file and remove from SQLite (write-through).
    async fn delete_memory(&self, scenario: Scenario, filename: &str) -> Result<()>;
}
