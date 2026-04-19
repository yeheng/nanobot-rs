//! Write-through memory operations: CRUD + SQLite sync.
//!
//! `MemoryWriter` owns the write path — filesystem (SSOT) and SQLite
//! metadata/embedding upserts. All mutations go through this single type,
//! ensuring write-through consistency.

use anyhow::Result;
use gasket_storage::memory::*;
use tracing::{info, warn};

use super::types::ReindexReport;

/// Write-through memory mutation engine.
///
/// Holds the filesystem store, index scanner, SQLite metadata/embedding stores,
/// and the embedder. Every write updates the file first (atomic), then SQLite
/// (best-effort, recoverable via reindex).
pub(crate) struct MemoryWriter {
    store: FileMemoryStore,
    index_manager: FileIndexManager,
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
    embedder: Box<dyn Embedder>,
}

impl MemoryWriter {
    /// Assemble from pre-built components.
    pub fn new(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        embedder: Box<dyn Embedder>,
    ) -> Self {
        Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            embedder,
        }
    }

    /// Access the underlying filesystem store (for init/read in loader).
    pub fn store(&self) -> &FileMemoryStore {
        &self.store
    }

    /// Access the metadata store (for tests).
    #[cfg(test)]
    pub fn metadata_store(&self) -> &MetadataStore {
        &self.metadata_store
    }

    // ── Sync & Reindex ─────────────────────────────────────────────────

    /// Sync filesystem metadata into SQLite for all scenarios.
    pub async fn sync_all(&self) -> Result<()> {
        for scenario in Scenario::all() {
            let entries = self.index_manager.scan_entries(*scenario).await?;
            self.metadata_store
                .sync_entries(*scenario, &entries)
                .await?;
        }
        Ok(())
    }

    /// Full reindex: wipe SQLite cache and rebuild from filesystem (SSOT).
    pub async fn reindex(&self) -> Result<ReindexReport> {
        info!("Starting full memory reindex (destructive)");

        for scenario in Scenario::all() {
            self.metadata_store.delete_scenario(*scenario).await?;
        }
        self.embedding_store.delete_all().await?;
        info!("Cleared all SQLite metadata and embedding caches");

        let mut total_files = 0usize;
        let total_errors = 0usize;

        for scenario in Scenario::all() {
            let entries = self.index_manager.scan_entries(*scenario).await?;
            let file_count = entries.len();
            self.metadata_store
                .sync_entries(*scenario, &entries)
                .await?;
            total_files += file_count;
            info!("Reindexed {}: {} files", scenario.dir_name(), file_count);
        }

        // Rebuild embeddings for all entries that need them.
        let needs_embedding = self.metadata_store.query_needs_embedding().await?;
        if !needs_embedding.is_empty() {
            info!(
                "Rebuilding embeddings for {} entries",
                needs_embedding.len()
            );
            for entry in needs_embedding {
                let body = match self.store.read(entry.scenario, &entry.filename).await {
                    Ok(memory_file) => memory_file.content,
                    Err(e) => {
                        warn!(
                            "Failed to read {}/{} for embedding rebuild: {}",
                            entry.scenario.dir_name(),
                            entry.filename,
                            e
                        );
                        continue;
                    }
                };

                match self.embedder.embed(&body).await {
                    Ok(embedding) => {
                        if let Err(e) = self
                            .embedding_store
                            .upsert(
                                &entry.filename,
                                entry.scenario.dir_name(),
                                &entry.tags,
                                entry.frequency,
                                &embedding,
                                entry.tokens,
                            )
                            .await
                        {
                            warn!(
                                "Failed to upsert embedding for {}/{}: {}",
                                entry.scenario.dir_name(),
                                entry.filename,
                                e
                            );
                        } else if let Err(e) = self
                            .metadata_store
                            .mark_embedding_done(entry.scenario, &entry.filename)
                            .await
                        {
                            warn!(
                                "Failed to mark embedding done for {}/{}: {}",
                                entry.scenario.dir_name(),
                                entry.filename,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Embedding failed for {}/{}: {}",
                            entry.scenario.dir_name(),
                            entry.filename,
                            e
                        );
                    }
                }
            }
        }

        info!(
            "Reindex complete: {} files, {} errors",
            total_files, total_errors
        );
        Ok(ReindexReport {
            total_files,
            total_errors,
        })
    }

    // ── CRUD ────────────────────────────────────────────────────────────

    /// Create a new memory file and synchronously update SQLite (write-through).
    pub async fn create_memory(
        &self,
        scenario: Scenario,
        title: &str,
        tags: &[String],
        frequency: Frequency,
        content: &str,
    ) -> Result<String> {
        let id = format!("mem_{}", uuid::Uuid::now_v7());
        let now = chrono::Utc::now().to_rfc3339();
        let tokens = content.len() / 4;

        let meta = MemoryMeta {
            id: id.clone(),
            title: title.to_string(),
            r#type: "note".to_string(),
            scenario,
            tags: tags.to_vec(),
            frequency,
            access_count: 0,
            created: now.clone(),
            updated: now.clone(),
            last_accessed: now,
            auto_expire: false,
            expires: None,
            tokens,
            superseded_by: None,
            index: true,
        };

        let filename = format!("{}.md", meta.id);
        let file_content = serialize_memory_file(&meta, content);

        // 1. Write file atomically (SSOT — must succeed)
        self.store
            .update(scenario, &filename, &file_content)
            .await?;

        // 2. Best-effort SQLite sync (recoverable via reindex/watcher)
        if let Err(e) = self
            .sync_memory_to_db(scenario, &filename, &meta, content)
            .await
        {
            warn!(
                "SQLite sync failed for {}/{} (file is safe, reindex will recover): {}",
                scenario.dir_name(),
                filename,
                e
            );
        }

        info!(
            "Created memory: {}/{} ({} tokens)",
            scenario.dir_name(),
            filename,
            tokens
        );
        Ok(filename)
    }

    /// Update an existing memory file and synchronously update SQLite (write-through).
    pub async fn update_memory(
        &self,
        scenario: Scenario,
        filename: &str,
        content: &str,
    ) -> Result<()> {
        let existing = self.store.read(scenario, filename).await?;
        let mut meta = existing.metadata;

        let now = chrono::Utc::now().to_rfc3339();
        meta.updated = now.clone();
        meta.last_accessed = now;
        meta.tokens = content.len() / 4;

        let file_content = serialize_memory_file(&meta, content);

        self.store.update(scenario, filename, &file_content).await?;

        if let Err(e) = self
            .sync_memory_to_db(scenario, filename, &meta, content)
            .await
        {
            warn!(
                "SQLite sync failed for {}/{} (file is safe, reindex will recover): {}",
                scenario.dir_name(),
                filename,
                e
            );
        }

        info!("Updated memory: {}/{}", scenario.dir_name(), filename);
        Ok(())
    }

    /// Delete a memory file and synchronously remove from SQLite (write-through).
    pub async fn delete_memory(&self, scenario: Scenario, filename: &str) -> Result<()> {
        self.store.delete(scenario, filename).await?;

        self.metadata_store
            .delete_by_scenario_and_path(scenario, filename)
            .await?;

        let path_str = format!("{}/{}", scenario.dir_name(), filename);
        self.embedding_store.delete(&path_str).await?;

        info!("Deleted memory: {}/{}", scenario.dir_name(), filename);
        Ok(())
    }

    // ── Private helpers ─────────────────────────────────────────────────

    /// Synchronize a single memory file's metadata + embedding into SQLite.
    async fn sync_memory_to_db(
        &self,
        scenario: Scenario,
        filename: &str,
        meta: &MemoryMeta,
        content: &str,
    ) -> Result<()> {
        let file_path = self
            .store
            .base_dir()
            .join(scenario.dir_name())
            .join(filename);
        let (file_mtime, file_size) = tokio::fs::metadata(&file_path)
            .await
            .map(|m| {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                (mtime, m.len())
            })
            .unwrap_or((0, 0));

        let entry = MemoryIndexEntry {
            id: meta.id.clone(),
            title: meta.title.clone(),
            memory_type: meta.r#type.clone(),
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.to_string(),
            updated: meta.updated.clone(),
            scenario,
            last_accessed: meta.last_accessed.clone(),
            access_count: meta.access_count,
            file_mtime,
            file_size,
            needs_embedding: true,
        };
        self.metadata_store.upsert_entry(&entry).await?;

        match self.embedder.embed(content).await {
            Ok(embedding) => {
                self.embedding_store
                    .upsert(
                        filename,
                        scenario.dir_name(),
                        &meta.tags,
                        meta.frequency,
                        &embedding,
                        meta.tokens as u32,
                    )
                    .await?;

                self.metadata_store
                    .mark_embedding_done(scenario, filename)
                    .await?;
            }
            Err(e) => {
                warn!(
                    "Embedding failed for {}/{} (needs_embedding=true, reindex will retry): {}",
                    scenario.dir_name(),
                    filename,
                    e
                );
            }
        }

        Ok(())
    }
}
