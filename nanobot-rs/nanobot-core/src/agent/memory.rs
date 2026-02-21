//! Memory store for long-term context
//!
//! This module wraps the generic `memory::FileMemoryStore` to provide the
//! high-level `read_long_term`, `write_long_term`, `read_history`, and
//! `append_history` API used by the agent loop.

use std::path::PathBuf;

use anyhow::Result;

use crate::memory::{FileMemoryStore, MemoryStore as MemoryStoreTrait};

/// Memory store for long-term context.
///
/// Delegates to [`FileMemoryStore`] internally.
pub struct MemoryStore {
    store: FileMemoryStore,
}

impl MemoryStore {
    /// Create a new memory store backed by the workspace directory.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            store: FileMemoryStore::new(workspace),
        }
    }

    /// Read long-term memory (`MEMORY.md`).
    pub async fn read_long_term(&self) -> Result<String> {
        Ok(self
            .store
            .read("MEMORY.md")
            .await?
            .unwrap_or_default())
    }

    /// Write long-term memory (`MEMORY.md`).
    pub async fn write_long_term(&self, content: &str) -> Result<()> {
        self.store.write("MEMORY.md", content).await
    }

    /// Read history (`HISTORY.md`).
    pub async fn read_history(&self) -> Result<String> {
        Ok(self
            .store
            .read("HISTORY.md")
            .await?
            .unwrap_or_default())
    }

    /// Append to history (`HISTORY.md`).
    pub async fn append_history(&self, entry: &str) -> Result<()> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let content = format!("\n[{}] {}\n", timestamp, entry);
        self.store.append("HISTORY.md", &content).await
    }

    /// Get a reference to the underlying `FileMemoryStore`.
    pub fn inner(&self) -> &FileMemoryStore {
        &self.store
    }
}
