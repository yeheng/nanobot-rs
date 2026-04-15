//! Memory manager facade with unified relevance scoring for context injection.
//!
//! The MemoryManager orchestrates storage-layer components (FileMemoryStore,
//! FileIndexManager, MetadataStore, RetrievalEngine) to load memories via a
//! unified scoring algorithm:
//!
//! - **Exempt scenarios** (Profile, Decisions, Reference): score = ∞ (always loaded)
//! - **Frequency coefficient**: Hot 1.5×, Warm 1.0×, Cold 0.5×, Archived 0.0
//! - **Similarity base**: vector search score (0–1), or 0.5 default
//! - **Final score** = similarity_base × frequency_coefficient
//!
//! Candidates are sorted by score and truncated by the hard token cap.
//!
//! # Write-Through Consistency
//!
//! Agent writes (`create_memory`, `update_memory`, `delete_memory`) synchronously
//! update both the filesystem (SSOT) and SQLite metadata/embeddings. The file
//! watcher uses mtime comparison to detect external edits.

use anyhow::Result;
use chrono::Utc;
use gasket_storage::memory::*;
use gasket_storage::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Boxed embedder trait object for dependency injection.
type BoxedEmbedder = Box<dyn Embedder>;

/// Facade orchestrating three-phase memory loading for agent loop.
pub struct MemoryManager {
    store: FileMemoryStore,
    index_manager: FileIndexManager,
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
    retrieval: RetrievalEngine,
    embedder: BoxedEmbedder,
    budget: TokenBudget,
    /// Lock-free channel for recording memory accesses.
    /// Sends access entries to a background worker that batches and flushes.
    access_tx: tokio::sync::mpsc::UnboundedSender<AccessEntry>,
    /// Shutdown signal for the access log background worker.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Background task handle for graceful shutdown.
    access_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl MemoryManager {
    /// Create a new MemoryManager with default components.
    ///
    /// # Arguments
    /// * `base_dir` - Memory base directory (e.g., ~/.gasket/memory/)
    /// * `pool` - SQLite connection pool for metadata and embedding stores
    /// * `embedder` - Embedder for computing memory embeddings
    pub async fn new(
        base_dir: PathBuf,
        pool: &SqlitePool,
        embedder: BoxedEmbedder,
    ) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
        let metadata_store = MetadataStore::new(pool.clone());
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(pool.clone()),
            EmbeddingStore::new(pool.clone()),
        );
        let budget = TokenBudget::default();

        let (access_tx, shutdown_tx, access_task) =
            Self::spawn_access_worker(metadata_store.clone());

        Ok(Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            embedder,
            budget,
            access_tx,
            shutdown_tx,
            access_task: std::sync::Mutex::new(access_task),
        })
    }

    /// Create a MemoryManager with custom token budget.
    pub async fn with_budget(
        base_dir: PathBuf,
        pool: &SqlitePool,
        budget: TokenBudget,
        embedder: BoxedEmbedder,
    ) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
        let metadata_store = MetadataStore::new(pool.clone());
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(pool.clone()),
            EmbeddingStore::new(pool.clone()),
        );

        let (access_tx, shutdown_tx, access_task) =
            Self::spawn_access_worker(metadata_store.clone());

        Ok(Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            embedder,
            budget,
            access_tx,
            shutdown_tx,
            access_task: std::sync::Mutex::new(access_task),
        })
    }

    /// Create a MemoryManager with pre-built components (for testing).
    pub fn with_components(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        retrieval: RetrievalEngine,
        embedder: BoxedEmbedder,
        budget: TokenBudget,
    ) -> Self {
        let (access_tx, shutdown_tx, access_task) =
            Self::spawn_access_worker(metadata_store.clone());

        Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            embedder,
            budget,
            access_tx,
            shutdown_tx,
            access_task: std::sync::Mutex::new(access_task),
        }
    }

    /// Spawn the background access log worker.
    ///
    /// Creates an MPSC channel for lock-free access recording and a watch
    /// channel for shutdown signaling. The worker batches entries in memory
    /// and flushes to disk when the threshold is reached.
    fn spawn_access_worker(
        metadata_store: MetadataStore,
    ) -> (
        tokio::sync::mpsc::UnboundedSender<AccessEntry>,
        tokio::sync::watch::Sender<bool>,
        Option<tokio::task::JoinHandle<()>>,
    ) {
        let (access_tx, access_rx) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(access_log_worker(access_rx, shutdown_rx, metadata_store));

        (access_tx, shutdown_tx, Some(handle))
    }

    /// Initialize memory system (create directories, sync metadata to SQLite).
    pub async fn init(&self) -> Result<()> {
        self.store.init().await?;
        self.sync_all().await?;
        Ok(())
    }

    /// Sync filesystem metadata into SQLite for all scenarios.
    ///
    /// Reads YAML frontmatter from all memory files and upserts into the
    /// `memory_metadata` SQLite table. Should be called at startup and when
    /// files change (via watcher).
    pub async fn sync_all(&self) -> Result<()> {
        for scenario in Scenario::all() {
            let entries = self.index_manager.scan_entries(*scenario).await?;
            self.metadata_store
                .sync_entries(*scenario, &entries)
                .await?;
        }
        Ok(())
    }

    // ========================================================================
    // Access tracking (lock-free channel + background write-behind flush)
    // ========================================================================

    /// Record a memory access — lock-free, non-blocking.
    ///
    /// Sends the access entry to the background worker via MPSC channel.
    /// The worker batches entries and flushes to disk when the threshold is
    /// reached. This method never blocks or awaits, making it safe on the
    /// hot LLM response path.
    fn record_access(&self, scenario: Scenario, filename: &str) {
        let entry = AccessEntry {
            scenario,
            filename: filename.to_string(),
            timestamp: Utc::now(),
        };
        let _ = self.access_tx.send(entry);
    }

    /// Flush any remaining access log entries on graceful shutdown.
    ///
    /// Sends a shutdown signal to the background worker, which flushes any
    /// remaining entries before exiting. Awaits the worker task to ensure
    /// all data is persisted before returning.
    pub async fn shutdown_flush(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let handle = { self.access_task.lock().unwrap().take() };
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        Ok(())
    }

    // ========================================================================
    // Write-through methods: agent writes that synchronously update SQLite
    // ========================================================================

    /// Create a new memory file and synchronously update SQLite (write-through).
    ///
    /// # Crash Recovery
    ///
    /// The filesystem is the SSOT (Single Source of Truth). File writes use
    /// `atomic_write` (temp-file + rename) so a crash never leaves a partial file.
    /// SQLite metadata/embedding upserts are best-effort after the file is durable —
    /// if they fail, `reindex` or the file watcher will catch up on next startup.
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
    ///
    /// Same crash-recovery semantics as `create_memory`: file write is atomic,
    /// SQLite sync is best-effort after the file is durable.
    pub async fn update_memory(
        &self,
        scenario: Scenario,
        filename: &str,
        content: &str,
    ) -> Result<()> {
        // 1. Read existing file to get current metadata
        let existing = self.store.read(scenario, filename).await?;
        let mut meta = existing.metadata;

        // 2. Update metadata fields
        let now = chrono::Utc::now().to_rfc3339();
        meta.updated = now.clone();
        meta.last_accessed = now;
        meta.tokens = content.len() / 4;

        let file_content = serialize_memory_file(&meta, content);

        // 3. Write file atomically (SSOT — must succeed)
        self.store.update(scenario, filename, &file_content).await?;

        // 4. Best-effort SQLite sync (recoverable via reindex/watcher)
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
        // 1. Delete file from disk
        self.store.delete(scenario, filename).await?;

        // 2. Delete from SQLite metadata
        self.metadata_store
            .delete_by_scenario_and_path(scenario, filename)
            .await?;

        // 3. Delete embedding
        let path_str = format!("{}/{}", scenario.dir_name(), filename);
        self.embedding_store.delete(&path_str).await?;

        info!("Deleted memory: {}/{}", scenario.dir_name(), filename);
        Ok(())
    }

    /// Synchronize a single memory file's metadata + embedding into SQLite.
    ///
    /// Extracted from `create_memory` / `update_memory` to eliminate duplication.
    /// Returns an error so callers can decide whether to fail or degrade gracefully.
    async fn sync_memory_to_db(
        &self,
        scenario: Scenario,
        filename: &str,
        meta: &MemoryMeta,
        content: &str,
    ) -> Result<()> {
        // 1. Read file mtime for cache invalidation
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

        // 2. Upsert metadata into SQLite (needs_embedding = true until embedding succeeds)
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

        // 3. Compute embedding — if this fails, leave needs_embedding = true
        //    so reindex/refresh can retry later.
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

                // Mark embedding as complete
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

    /// Full reindex: wipe SQLite cache and rebuild from filesystem (SSOT).
    ///
    /// This is a **destructive, idempotent** operation:
    /// 1. Clears all metadata entries from `memory_metadata`
    /// 2. Clears all entries from `memory_embeddings`
    /// 3. Re-scans every scenario directory from disk
    /// 4. Parses YAML frontmatter (leniently — skips malformed files)
    /// 5. Rebuilds metadata and re-queues embeddings
    ///
    /// Since Markdown files are the single source of truth and SQLite is just
    /// a volatile cache, destroying and rebuilding the cache is always safe.
    pub async fn reindex(&self) -> Result<ReindexReport> {
        info!("Starting full memory reindex (destructive)");

        // 1. Wipe the entire SQLite cache — it will be rebuilt from disk
        for scenario in Scenario::all() {
            self.metadata_store.delete_scenario(*scenario).await?;
        }
        self.embedding_store.delete_all().await?;
        info!("Cleared all SQLite metadata and embedding caches");

        // 2. Re-scan all directories and rebuild from filesystem
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

        info!(
            "Reindex complete: {} files, {} errors",
            total_files, total_errors
        );
        Ok(ReindexReport {
            total_files,
            total_errors,
        })
    }

    // ========================================================================
    // Unified memory loading with relevance scoring
    // ========================================================================

    /// When a file read fails because the file was deleted externally,
    /// clean up the stale SQLite metadata + embedding entries on-the-fly.
    ///
    /// Returns `true` if the error was `NotFound` (caller should skip).
    async fn cleanup_stale_if_not_found(
        &self,
        scenario: Scenario,
        filename: &str,
        error: &anyhow::Error,
    ) -> bool {
        if let Some(io_err) = error.root_cause().downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::NotFound {
                warn!(
                    "File gone from disk, cleaning stale SQLite entry: {}/{}",
                    scenario.dir_name(),
                    filename
                );
                let _ = self
                    .metadata_store
                    .delete_by_scenario_and_path(scenario, filename)
                    .await;
                let _ = self
                    .embedding_store
                    .delete(&format!("{}/{}", scenario.dir_name(), filename))
                    .await;
                return true;
            }
        }
        false
    }

    /// Load memories for context injection using unified relevance scoring.
    ///
    /// Replaces the former three-phase loading with a single scoring pass:
    /// - Exempt scenarios (Profile, Decisions, Reference): score = ∞
    /// - Frequency coefficient: Hot 1.5×, Warm 1.0×, Cold 0.5×, Archived 0.0
    /// - Similarity base: vector search score (0–1), or 0.5 default
    /// - Final score = similarity_base × frequency_coefficient
    ///
    /// Candidates are sorted by score and truncated by the hard token cap.
    pub async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext> {
        let scenario = query.scenario.unwrap_or(Scenario::Knowledge);
        let mut seen = HashSet::new();
        let mut candidates: Vec<ScoredCandidate> = Vec::new();

        // 1. Collect from metadata store (profile + active + scenario hot/warm)
        let entries = self
            .metadata_store
            .query_for_loading(scenario, &query.tags)
            .await
            .unwrap_or_default();

        for entry in &entries {
            let key = format!("{}/{}", entry.scenario.dir_name(), entry.filename);
            if seen.insert(key) {
                let score = ScoredCandidate::compute(entry.scenario, entry.frequency, None);
                if score > 0.0 {
                    candidates.push(ScoredCandidate {
                        scenario: entry.scenario,
                        filename: entry.filename.clone(),
                        tokens: entry.tokens,
                        score,
                    });
                }
            }
        }

        // 2. Augment with vector search results (if query text provided)
        if query.text.is_some() {
            if let Ok(results) = self.retrieval.search(query).await {
                for result in &results {
                    let key = format!("{}/{}", result.scenario.dir_name(), result.memory_path);
                    if seen.insert(key) {
                        let score = ScoredCandidate::compute(
                            result.scenario,
                            result.frequency,
                            Some(result.score),
                        );
                        if score > 0.0 {
                            candidates.push(ScoredCandidate {
                                scenario: result.scenario,
                                filename: result.memory_path.clone(),
                                tokens: result.tokens,
                                score,
                            });
                        }
                    }
                }
            }
        }

        // 3. Sort by score descending (exempt ∞ items first, then by score)
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 4. Load files within token budget
        let mut memories = Vec::new();
        let mut tokens_used = 0;
        let cap = self.budget.total_cap;

        for candidate in &candidates {
            if tokens_used + candidate.tokens as usize > cap {
                break;
            }
            match self
                .store
                .read(candidate.scenario, &candidate.filename)
                .await
            {
                Ok(mem) => {
                    tokens_used += mem.metadata.tokens;
                    self.record_access(candidate.scenario, &candidate.filename);
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(candidate.scenario, &candidate.filename, &e)
                        .await
                    {
                        warn!(
                            "Failed to load {}/{}: {}",
                            candidate.scenario.dir_name(),
                            candidate.filename,
                            e
                        );
                    }
                }
            }
        }

        Ok(MemoryContext {
            memories,
            tokens_used,
        })
    }
}

/// A candidate memory with computed relevance score for unified ranking.
struct ScoredCandidate {
    scenario: Scenario,
    filename: String,
    tokens: u32,
    score: f32,
}

impl ScoredCandidate {
    /// Compute relevance score for a memory candidate.
    ///
    /// Scoring formula: `similarity_base × frequency_coefficient`
    /// - Exempt scenarios (profile, decisions, reference): ∞ (always first)
    /// - Archived frequency: 0.0 (excluded)
    fn compute(scenario: Scenario, frequency: Frequency, similarity: Option<f32>) -> f32 {
        if scenario.is_exempt_from_decay() {
            return f32::INFINITY;
        }
        if matches!(frequency, Frequency::Archived) {
            return 0.0;
        }
        let base = similarity.unwrap_or(0.5);
        let coeff = match frequency {
            Frequency::Hot => 1.5,
            Frequency::Warm => 1.0,
            Frequency::Cold => 0.5,
            Frequency::Archived => 0.0,
        };
        base * coeff
    }
}

// ── Background access log worker ──────────────────────────────────────────

/// Background worker that receives access entries from the MPSC channel,
/// batches them in memory, and flushes to disk when the threshold is reached.
///
/// On shutdown signal (watch channel) or channel closure, flushes any remaining
/// entries before exiting.
async fn access_log_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AccessEntry>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    metadata_store: MetadataStore,
) {
    let mut log = AccessLog::default_threshold();

    loop {
        tokio::select! {
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        log.record(entry.scenario, &entry.filename);
                        if log.should_flush() {
                            match FrequencyManager::flush_access_log(
                                &mut log, &metadata_store,
                            )
                            .await
                            {
                                Ok(report) if report.total_flushed > 0 => {
                                    debug!(
                                        "Access log flushed: {} files updated, {} promoted",
                                        report.total_flushed, report.promoted
                                    );
                                }
                                Err(e) => warn!("Access log flush failed: {}", e),
                                _ => {}
                            }
                        }
                    }
                    None => break, // Channel closed — all senders dropped
                }
            }
            _ = shutdown_rx.changed() => break, // Shutdown signal received
        }
    }

    // Final flush on shutdown — drain remaining entries to disk
    if !log.is_empty() {
        info!(
            "Flushing {} remaining access log entries on shutdown",
            log.len()
        );
        if let Err(e) = FrequencyManager::flush_access_log(&mut log, &metadata_store).await {
            warn!("Shutdown flush failed: {}", e);
        }
    }
}

/// Result of a full reindex operation.
#[derive(Debug)]
pub struct ReindexReport {
    pub total_files: usize,
    pub total_errors: usize,
}

/// Result of loading memories for context injection.
#[derive(Debug)]
pub struct MemoryContext {
    /// Loaded memory files (within token budget).
    pub memories: Vec<MemoryFile>,
    /// Total tokens used.
    pub tokens_used: usize,
}

/// Implement MemoryProvider trait for MemoryManager.
///
/// This allows HistoryCoordinator to depend on the trait rather than
/// the concrete type, enabling test doubles and future alternative backends.
#[async_trait::async_trait]
impl super::store::MemoryProvider for MemoryManager {
    async fn load_for_context(&self, query: &MemoryQuery) -> anyhow::Result<MemoryContext> {
        self.load_for_context(query).await
    }

    async fn search(&self, query: &str, top_k: usize) -> anyhow::Result<Vec<MemoryHit>> {
        let memory_query = MemoryQuery {
            text: Some(query.to_string()),
            tags: vec![],
            scenario: None,
            max_tokens: Some(top_k * 200),
        };
        let ctx = self.load_for_context(&memory_query).await?;
        let hits: Vec<MemoryHit> = ctx
            .memories
            .into_iter()
            .map(|m| MemoryHit {
                path: format!("{}/{}", m.metadata.scenario.dir_name(), m.metadata.id),
                scenario: m.metadata.scenario,
                title: m.metadata.title.clone(),
                tags: m.metadata.tags,
                frequency: m.metadata.frequency,
                score: 0.0,
                tokens: m.metadata.tokens,
            })
            .take(top_k)
            .collect();
        Ok(hits)
    }

    async fn create_memory(
        &self,
        scenario: Scenario,
        _filename: &str,
        title: &str,
        tags: &[String],
        frequency: Frequency,
        content: &str,
    ) -> anyhow::Result<()> {
        self.create_memory(scenario, title, tags, frequency, content)
            .await?;
        Ok(())
    }

    async fn update_memory(
        &self,
        scenario: Scenario,
        filename: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.update_memory(scenario, filename, content).await
    }

    async fn delete_memory(&self, scenario: Scenario, filename: &str) -> anyhow::Result<()> {
        self.delete_memory(scenario, filename).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_storage::memory::{serialize_memory_file, MemoryMeta};
    use gasket_storage::SqliteStore;
    use tempfile::TempDir;

    /// Create a test memory file with proper frontmatter.
    async fn create_memory_file(
        base_dir: &PathBuf,
        scenario: Scenario,
        filename: &str,
        title: &str,
        tags: &[&str],
        frequency: Frequency,
        tokens: usize,
    ) -> Result<()> {
        let dir = base_dir.join(scenario.dir_name());
        tokio::fs::create_dir_all(&dir).await?;

        let meta = MemoryMeta {
            id: format!("mem_{}", uuid::Uuid::new_v4()),
            title: title.to_string(),
            r#type: "test".to_string(),
            scenario,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            frequency,
            access_count: 0,
            created: chrono::Utc::now().to_rfc3339(),
            updated: chrono::Utc::now().to_rfc3339(),
            last_accessed: chrono::Utc::now().to_rfc3339(),
            auto_expire: false,
            expires: None,
            tokens,
            superseded_by: None,
            index: true,
        };

        let content = format!("# {}\n\nTest content for {}", title, title);
        let mut file_content = serialize_memory_file(&meta, &content);

        // Re-inject frequency into YAML frontmatter.
        // serialize_memory_file skips it (SQLite-only in production),
        // but parse_frontmatter can still read it — this lets scan_entries
        // pick up the correct frequency for test files.
        let freq_str = format!("{:?}", meta.frequency).to_lowercase();
        file_content = file_content.replace(
            "auto_expire:",
            &format!("frequency: {}\nauto_expire:", freq_str),
        );

        tokio::fs::write(dir.join(filename), file_content).await?;
        Ok(())
    }

    /// Setup a test memory manager with temp directory and in-memory SQLite.
    async fn setup_manager() -> (MemoryManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().join("memory");
        tokio::fs::create_dir_all(&base_dir).await.unwrap();

        // Create SQLite pool
        let db_path = temp_dir.path().join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        // Use NoopEmbedder for tests (zero vectors, no model loading)
        let embedder: BoxedEmbedder = Box::new(NoopEmbedder::new(384));
        let manager = MemoryManager::new(base_dir, &pool, embedder).await.unwrap();
        manager.init().await.unwrap();

        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_bootstrap_loads_by_frequency_not_filename() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create profile files (always loaded)
        create_memory_file(
            &memory_dir,
            Scenario::Profile,
            "user.md",
            "User Profile",
            &["profile"],
            Frequency::Hot,
            100,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Profile,
            "prefs.md",
            "Preferences",
            &["settings"],
            Frequency::Hot,
            150,
        )
        .await
        .unwrap();

        // Create active files with Hot/Warm frequency — names don't matter
        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "current_project.md",
            "Current Project",
            &["active"],
            Frequency::Hot,
            200,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "backlog_tasks.md",
            "Backlog Tasks",
            &["active"],
            Frequency::Warm,
            250,
        )
        .await
        .unwrap();

        // Create active file with Cold frequency — should NOT be in bootstrap
        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "old_idea.md",
            "Old Idea",
            &["active"],
            Frequency::Cold,
            100,
        )
        .await
        .unwrap();

        // Create knowledge file — should NOT be in bootstrap (Phase 1)
        // Cold frequency so it won't auto-load in Phase 2 either
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "test.md",
            "Test Knowledge",
            &["test"],
            Frequency::Cold,
            100,
        )
        .await
        .unwrap();

        // Sync newly created files to SQLite
        manager.sync_all().await.unwrap();

        // Load with empty query
        let query = MemoryQuery::new();
        let context = manager.load_for_context(&query).await.unwrap();

        // Should load profile (2) + active Hot/Warm (2) = 4 files
        // Cold active file and Cold knowledge file excluded from bootstrap
        assert_eq!(context.memories.len(), 4);
        assert_eq!(context.tokens_used, 700);

        // Verify Cold file was excluded
        assert!(!context
            .memories
            .iter()
            .any(|m| m.metadata.title == "Old Idea"));
    }

    #[tokio::test]
    async fn test_bootstrap_loads_arbitrary_named_hot_files() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create active files with arbitrary names — only frequency matters
        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "todo.md", // NOT "current*" or "backlog*"
            "Todo List",
            &["active"],
            Frequency::Hot,
            100,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "sprint_plan.md", // NOT "current*" or "backlog*"
            "Sprint Plan",
            &["active"],
            Frequency::Warm,
            150,
        )
        .await
        .unwrap();

        // Sync newly created files to SQLite
        manager.sync_all().await.unwrap();

        let query = MemoryQuery::new();
        let context = manager.load_for_context(&query).await.unwrap();

        // Both files should load regardless of filename
        assert_eq!(context.memories.len(), 2);
        assert!(context
            .memories
            .iter()
            .any(|m| m.metadata.title == "Todo List"));
        assert!(context
            .memories
            .iter()
            .any(|m| m.metadata.title == "Sprint Plan"));
        assert_eq!(context.tokens_used, 250);
    }

    #[tokio::test]
    async fn test_scenario_phase_respects_budget() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Regenerate index first (empty)
        manager.sync_all().await.unwrap();

        // Create knowledge files exceeding budget (1500)
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "hot1.md",
            "Hot 1",
            &["rust"],
            Frequency::Hot,
            800,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "hot2.md",
            "Hot 2",
            &["rust"],
            Frequency::Hot,
            800,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "warm1.md",
            "Warm 1",
            &["rust"],
            Frequency::Warm,
            500,
        )
        .await
        .unwrap();

        // Sync metadata to SQLite
        manager.sync_all().await.unwrap();

        // Load with scenario filter
        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_tag("rust");
        let context = manager.load_for_context(&query).await.unwrap();

        // With unified scoring, total_cap (4000) is the only limit.
        // Both hot items (800+800=1600) should fit within budget.
        assert!(context.tokens_used <= 4000);
        assert!(context.memories.iter().any(|m| m.metadata.title == "Hot 1"));
        assert!(context.memories.iter().any(|m| m.metadata.title == "Hot 2"));
    }

    #[tokio::test]
    async fn test_on_demand_fills_remaining() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Sync metadata to SQLite
        manager.sync_all().await.unwrap();

        // Create knowledge files with cold frequency (not loaded in phase 2)
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "cold1.md",
            "Cold 1",
            &["search"],
            Frequency::Cold,
            500,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "cold2.md",
            "Cold 2",
            &["search"],
            Frequency::Cold,
            400,
        )
        .await
        .unwrap();

        // Sync metadata to SQLite
        manager.sync_all().await.unwrap();

        // Load with text query (triggers on-demand)
        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_text("search query")
            .with_tag("search");
        let context = manager.load_for_context(&query).await.unwrap();

        // Cold items should be found by search and loaded (score > 0)
        assert!(
            context.tokens_used > 0,
            "Unified scoring should load cold items from search"
        );
    }

    #[tokio::test]
    async fn test_total_never_exceeds_hard_cap() {
        let budget = TokenBudget {
            bootstrap: 500,
            scenario: 500,
            on_demand: 500,
            total_cap: 1000,
        };

        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create large files
        create_memory_file(
            &memory_dir,
            Scenario::Profile,
            "profile1.md",
            "Profile 1",
            &[],
            Frequency::Hot,
            600,
        )
        .await
        .unwrap();
        create_memory_file(
            &memory_dir,
            Scenario::Profile,
            "profile2.md",
            "Profile 2",
            &[],
            Frequency::Hot,
            600,
        )
        .await
        .unwrap();

        manager.sync_all().await.unwrap();

        // Create manager with custom budget
        let store = FileMemoryStore::new(memory_dir.clone());
        let index_manager = FileIndexManager::new(memory_dir.clone());
        let pool = SqliteStore::with_path(temp_dir.path().join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let embedding_store = EmbeddingStore::new(pool.clone());
        let metadata_store = MetadataStore::new(pool);
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(
                SqliteStore::with_path(temp_dir.path().join("test.db"))
                    .await
                    .unwrap()
                    .pool()
                    .clone(),
            ),
            EmbeddingStore::new(
                SqliteStore::with_path(temp_dir.path().join("test.db"))
                    .await
                    .unwrap()
                    .pool()
                    .clone(),
            ),
        );
        let embedder: BoxedEmbedder = Box::new(NoopEmbedder::new(384));
        let custom_manager = MemoryManager::with_components(
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            embedder,
            budget,
        );

        let query = MemoryQuery::new();
        let context = custom_manager.load_for_context(&query).await.unwrap();

        // Should never exceed hard cap
        assert!(context.tokens_used <= 1000);
    }

    #[tokio::test]
    async fn test_graceful_skip_on_corrupted_file() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create valid file
        create_memory_file(
            &memory_dir,
            Scenario::Profile,
            "valid.md",
            "Valid",
            &[],
            Frequency::Hot,
            100,
        )
        .await
        .unwrap();

        // Create corrupted file
        let corrupted_path = memory_dir.join("profile/corrupted.md");
        tokio::fs::write(&corrupted_path, "invalid frontmatter\n---\ncontent")
            .await
            .unwrap();

        // Sync newly created files to SQLite
        manager.sync_all().await.unwrap();

        // Should load valid file and skip corrupted
        let query = MemoryQuery::new();
        let context = manager.load_for_context(&query).await.unwrap();

        assert_eq!(context.memories.len(), 1);
        assert_eq!(context.memories[0].metadata.title, "Valid");
        assert_eq!(context.tokens_used, 100);
    }

    #[tokio::test]
    async fn test_deduplication_across_phases() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create a file in knowledge
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "duplicate.md",
            "Duplicate",
            &["test"],
            Frequency::Hot,
            100,
        )
        .await
        .unwrap();
        manager.sync_all().await.unwrap();

        // The file should only load once (in scenario phase, not again in on-demand)
        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_text("duplicate")
            .with_tag("test");
        let context = manager.load_for_context(&query).await.unwrap();

        // Count how many times the file appears
        let count = context
            .memories
            .iter()
            .filter(|m| m.metadata.title == "Duplicate")
            .count();
        assert_eq!(count, 1, "File should only load once");
    }

    #[tokio::test]
    async fn test_create_memory_write_through() {
        let (manager, _temp_dir) = setup_manager().await;

        let filename = manager
            .create_memory(
                Scenario::Knowledge,
                "Test Write-Through",
                &["test".to_string()],
                Frequency::Hot,
                "This is test content for write-through",
            )
            .await
            .unwrap();

        // File should exist on disk
        assert!(!filename.is_empty());

        // SQLite should have the entry immediately
        let entries = manager
            .metadata_store
            .query_entries(Scenario::Knowledge)
            .await
            .unwrap();
        assert_eq!(1, entries.len());
        assert_eq!("Test Write-Through", entries[0].title);
        assert_eq!(Frequency::Hot, entries[0].frequency);
    }

    #[tokio::test]
    async fn test_delete_memory_write_through() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create a file first
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "to_delete.md",
            "To Delete",
            &["test"],
            Frequency::Warm,
            100,
        )
        .await
        .unwrap();
        manager.sync_all().await.unwrap();

        // Verify it exists
        let entries_before = manager
            .metadata_store
            .query_entries(Scenario::Knowledge)
            .await
            .unwrap();
        assert_eq!(1, entries_before.len());

        // Delete via write-through
        manager
            .delete_memory(Scenario::Knowledge, "to_delete.md")
            .await
            .unwrap();

        // SQLite should be empty
        let entries_after = manager
            .metadata_store
            .query_entries(Scenario::Knowledge)
            .await
            .unwrap();
        assert_eq!(0, entries_after.len());
    }

    #[tokio::test]
    async fn test_reindex() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        // Create some files
        create_memory_file(
            &memory_dir,
            Scenario::Knowledge,
            "reindex_test.md",
            "Reindex Test",
            &["test"],
            Frequency::Hot,
            100,
        )
        .await
        .unwrap();

        let report = manager.reindex().await.unwrap();
        assert_eq!(1, report.total_files);
        assert_eq!(0, report.total_errors);
    }
}
