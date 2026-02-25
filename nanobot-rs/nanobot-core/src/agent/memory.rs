//! Memory store for long-term context
//!
//! This module wraps `SqliteStore` to provide the high-level
//! `read_long_term`, `write_long_term`, `read_history`, and `append_history`
//! API used by the agent loop.

use anyhow::Result;
use tracing::instrument;

use crate::memory::{MemoryEntry, MemoryQuery, MemoryStore as MemoryStoreTrait, SqliteStore};

/// Memory store for long-term context.
///
/// Backed by `SqliteStore` for all operations:
/// - Structured memories (save/get/delete/search)
/// - Long-term memory (MEMORY.md equivalent via kv_store)
/// - History (via history table)
pub struct MemoryStore {
    store: SqliteStore,
}

impl MemoryStore {
    /// Create a new memory store.
    ///
    /// Opens the default `SqliteStore` at `~/.nanobot/memory.db`.
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

    // ── Structured memory API ──

    /// Save a structured memory entry.
    #[instrument(name = "memory.save", skip_all, fields(id = %entry.id))]
    pub async fn save(&self, entry: &MemoryEntry) -> Result<()> {
        self.store.save(entry).await
    }

    /// Retrieve a structured memory entry by id.
    #[instrument(name = "memory.get", skip_all)]
    pub async fn get(&self, id: &str) -> Result<Option<MemoryEntry>> {
        self.store.get(id).await
    }

    /// Delete a structured memory entry by id.
    #[instrument(name = "memory.delete", skip_all)]
    pub async fn delete(&self, id: &str) -> Result<bool> {
        self.store.delete(id).await
    }

    /// Search structured memories.
    #[instrument(name = "memory.search", skip_all)]
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>> {
        self.store.search(query).await
    }

    // ── Long-term memory API ──

    /// Read long-term memory (MEMORY.md equivalent).
    #[instrument(name = "memory.read_long_term", skip_all)]
    pub async fn read_long_term(&self) -> Result<String> {
        Ok(self.store.read_raw("MEMORY.md").await?.unwrap_or_default())
    }

    /// Write long-term memory (MEMORY.md equivalent).
    #[instrument(name = "memory.write_long_term", skip_all)]
    pub async fn write_long_term(&self, content: &str) -> Result<()> {
        self.store.write_raw("MEMORY.md", content).await
    }

    // ── History API ──

    /// Read history.
    #[instrument(name = "memory.read_history", skip_all)]
    pub async fn read_history(&self) -> Result<String> {
        self.store.read_history().await
    }

    /// Append to history.
    #[instrument(name = "memory.append_history", skip_all)]
    pub async fn append_history(&self, entry: &str) -> Result<()> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let content = format!("\n[{}] {}\n", timestamp, entry);
        self.store.append_history(&content).await
    }
}
