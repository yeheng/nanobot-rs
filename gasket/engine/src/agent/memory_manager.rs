//! Memory manager facade with three-phase loading for context injection.
//!
//! The MemoryManager orchestrates the storage-layer components (FileMemoryStore,
//! FileIndexManager, MetadataStore, RetrievalEngine) to provide three-phase
//! memory loading backed by SQLite metadata queries:
//!
//! - **Phase 1 (Bootstrap, ~700 tokens)**: Always loads profile + active memories
//! - **Phase 2 (Scenario, ~1500 tokens)**: Loads scenario-specific hot/warm items
//! - **Phase 3 (On-demand, ~1000 tokens)**: Fills remaining budget via search
//!
//! Total never exceeds the hard cap (default 3200 tokens).
//!
//! # Write-Through Consistency
//!
//! Agent writes (`create_memory`, `update_memory`, `delete_memory`) synchronously
//! update both the filesystem (SSOT) and SQLite metadata/embeddings. The file
//! watcher uses mtime comparison to detect external edits.

use anyhow::Result;
use gasket_storage::memory::*;
use gasket_storage::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Facade managing three-phase memory loading for agent loop.
pub struct MemoryManager {
    store: FileMemoryStore,
    index_manager: FileIndexManager,
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
    retrieval: RetrievalEngine,
    budget: TokenBudget,
}

impl MemoryManager {
    /// Create a new MemoryManager with default components.
    ///
    /// # Arguments
    /// * `base_dir` - Memory base directory (e.g., ~/.gasket/memory/)
    /// * `pool` - SQLite connection pool for metadata and embedding stores
    pub async fn new(base_dir: PathBuf, pool: &SqlitePool) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
        let metadata_store = MetadataStore::new(pool.clone());
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(pool.clone()),
            EmbeddingStore::new(pool.clone()),
        );
        let budget = TokenBudget::default();

        Ok(Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            budget,
        })
    }

    /// Create a MemoryManager with custom token budget.
    pub async fn with_budget(
        base_dir: PathBuf,
        pool: &SqlitePool,
        budget: TokenBudget,
    ) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
        let metadata_store = MetadataStore::new(pool.clone());
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(pool.clone()),
            EmbeddingStore::new(pool.clone()),
        );

        Ok(Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            budget,
        })
    }

    /// Create a MemoryManager with pre-built components (for testing).
    pub fn with_components(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        retrieval: RetrievalEngine,
        budget: TokenBudget,
    ) -> Self {
        Self {
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            budget,
        }
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
    // Write-through methods: agent writes that synchronously update SQLite
    // ========================================================================

    /// Create a new memory file and synchronously update SQLite (write-through).
    ///
    /// Writes the file to disk, reads file_mtime, then upserts metadata + embedding into SQLite.
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

        // 1. Write file atomically
        self.store
            .update(scenario, &filename, &file_content)
            .await?;

        // 2. Read file mtime
        let file_path = self
            .store
            .base_dir()
            .join(scenario.dir_name())
            .join(&filename);
        let file_mtime = tokio::fs::metadata(&file_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // 3. Upsert metadata into SQLite
        let entry = MemoryIndexEntry {
            id: meta.id,
            title: meta.title,
            memory_type: meta.r#type,
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.clone(),
            updated: meta.updated,
            scenario,
            last_accessed: meta.last_accessed,
            file_mtime,
        };
        self.metadata_store.upsert_entry(&entry).await?;

        // 4. Upsert embedding (placeholder vector until real embedder)
        let embedding = vec![0.0f32; 384];
        self.embedding_store
            .upsert(
                &filename,
                scenario.dir_name(),
                tags,
                frequency,
                &embedding,
                tokens as u32,
            )
            .await?;

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
    /// Reads the current file to preserve frontmatter metadata, updates the body,
    /// then re-upserts metadata + embedding into SQLite.
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

        // 3. Write file atomically (store.update handles version history)
        self.store.update(scenario, filename, &file_content).await?;

        // 4. Read file mtime
        let file_path = self
            .store
            .base_dir()
            .join(scenario.dir_name())
            .join(filename);
        let file_mtime = tokio::fs::metadata(&file_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // 5. Upsert metadata into SQLite
        let entry = MemoryIndexEntry {
            id: meta.id,
            title: meta.title,
            memory_type: meta.r#type,
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.to_string(),
            updated: meta.updated,
            scenario,
            last_accessed: meta.last_accessed,
            file_mtime,
        };
        self.metadata_store.upsert_entry(&entry).await?;

        // 6. Upsert embedding
        let embedding = vec![0.0f32; 384];
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

    /// Full reindex: scan all files from disk and rebuild SQLite metadata.
    ///
    /// Used by the CLI `gasket memory reindex` command to repair stale indexes.
    pub async fn reindex(&self) -> Result<ReindexReport> {
        info!("Starting full memory reindex");
        let mut total_files = 0usize;
        let mut total_errors = 0usize;

        for scenario in Scenario::all() {
            let entries = self.index_manager.scan_entries(*scenario).await?;
            let file_count = entries.len();
            let error_count = entries
                .iter()
                .filter(|e| e.frequency == Frequency::Archived && e.title.starts_with("[broken]"))
                .count();

            self.metadata_store
                .sync_entries(*scenario, &entries)
                .await?;

            total_files += file_count;
            total_errors += error_count;
            info!(
                "Reindexed {}: {} files, {} errors",
                scenario.dir_name(),
                file_count,
                error_count
            );
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
    // Three-phase loading
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

    /// Combined three-phase loading respecting total budget.
    ///
    /// # Arguments
    /// * `query` - Memory query with optional text, tags, scenario filter
    ///
    /// # Returns
    /// * `MemoryContext` with loaded memories and token breakdown
    pub async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext> {
        let mut loaded = Vec::new();
        let mut loaded_filenames = HashSet::new();

        let mut on_demand_tokens = 0;

        // Phase 1: Bootstrap (always loaded)
        debug!("Phase 1: Loading bootstrap memories");
        let bootstrap_memories = self.load_bootstrap(&mut loaded_filenames).await?;
        let bootstrap_tokens = bootstrap_memories.iter().map(|m| m.metadata.tokens).sum();
        loaded.extend(bootstrap_memories);
        debug!("Phase 1 complete: {} tokens", bootstrap_tokens);

        // Phase 2: Scenario-specific
        let scenario = query.scenario.unwrap_or(Scenario::Knowledge);
        debug!("Phase 2: Loading scenario memories for {:?}", scenario);
        let scenario_memories = self
            .load_scenario(scenario, &query.tags, &mut loaded_filenames)
            .await?;
        let scenario_tokens = scenario_memories.iter().map(|m| m.metadata.tokens).sum();
        loaded.extend(scenario_memories);
        debug!("Phase 2 complete: {} tokens", scenario_tokens);

        // Phase 3: On-demand search
        let remaining = self
            .budget
            .total_cap
            .saturating_sub(bootstrap_tokens + scenario_tokens);
        if remaining > 0 {
            debug!(
                "Phase 3: Loading on-demand memories (budget: {})",
                remaining
            );
            let on_demand_memories = self
                .load_on_demand(query, remaining, &mut loaded_filenames)
                .await?;
            on_demand_tokens = on_demand_memories.iter().map(|m| m.metadata.tokens).sum();
            loaded.extend(on_demand_memories);
            debug!("Phase 3 complete: {} tokens", on_demand_tokens);
        }

        let total_used = bootstrap_tokens + scenario_tokens + on_demand_tokens;

        // Enforce hard cap — recalculate breakdown from actual truncated list
        if total_used > self.budget.total_cap {
            warn!(
                "Total tokens {} exceeds cap {}, truncating",
                total_used, self.budget.total_cap
            );
            let mut truncated = Vec::new();
            let mut accum = 0;
            for mem in loaded {
                if accum + mem.metadata.tokens > self.budget.total_cap {
                    break;
                }
                accum += mem.metadata.tokens;
                truncated.push(mem);
            }

            // Recalculate phase breakdown from the actual truncated list
            let mut actual_bootstrap = 0usize;
            let mut actual_scenario = 0usize;
            let mut actual_on_demand = 0usize;
            for mem in &truncated {
                match mem.metadata.scenario {
                    Scenario::Profile => actual_bootstrap += mem.metadata.tokens,
                    Scenario::Active => actual_bootstrap += mem.metadata.tokens,
                    _ => {
                        if mem.metadata.frequency == Frequency::Hot {
                            actual_scenario += mem.metadata.tokens;
                        } else {
                            actual_on_demand += mem.metadata.tokens;
                        }
                    }
                }
            }

            return Ok(MemoryContext {
                memories: truncated,
                tokens_used: accum,
                phase_breakdown: PhaseBreakdown {
                    bootstrap_tokens: actual_bootstrap,
                    scenario_tokens: actual_scenario,
                    on_demand_tokens: actual_on_demand,
                },
            });
        }

        Ok(MemoryContext {
            memories: loaded,
            tokens_used: total_used,
            phase_breakdown: PhaseBreakdown {
                bootstrap_tokens,
                scenario_tokens,
                on_demand_tokens,
            },
        })
    }

    /// Phase 1: Load profile + active memories (always loaded).
    ///
    /// Uses SQLite metadata queries instead of filesystem scanning.
    /// - All profile entries (always high-priority)
    /// - Active entries: Hot first, then Warm if budget allows
    /// - Skips Cold and Archived items
    async fn load_bootstrap(&self, loaded: &mut HashSet<String>) -> Result<Vec<MemoryFile>> {
        let mut memories = Vec::new();
        let budget = self.budget.bootstrap;
        let mut tokens_used = 0;

        // Load all profile entries from SQLite
        let profile_entries = self
            .metadata_store
            .query_entries(Scenario::Profile)
            .await
            .unwrap_or_default();

        for entry in &profile_entries {
            let key = format!("profile/{}", entry.filename);
            if !loaded.insert(key) {
                continue;
            }
            match self.store.read(Scenario::Profile, &entry.filename).await {
                Ok(mem) => {
                    tokens_used += mem.metadata.tokens;
                    debug!(
                        "Bootstrap: loaded profile/{} ({} tokens)",
                        entry.filename, mem.metadata.tokens
                    );
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(Scenario::Profile, &entry.filename, &e)
                        .await
                    {
                        warn!(
                            "Bootstrap: failed to load profile/{}: {}",
                            entry.filename, e
                        );
                    }
                }
            }
        }

        // Load active entries from SQLite (already sorted: Hot -> Warm -> Cold)
        let active_entries = self
            .metadata_store
            .query_entries(Scenario::Active)
            .await
            .unwrap_or_default();

        for entry in &active_entries {
            let key = format!("active/{}", entry.filename);
            if loaded.contains(&key) {
                continue;
            }

            // Skip Cold and Archived in bootstrap
            if matches!(entry.frequency, Frequency::Cold | Frequency::Archived) {
                continue;
            }

            if tokens_used + entry.tokens as usize > budget {
                break;
            }

            match self.store.read(Scenario::Active, &entry.filename).await {
                Ok(mem) => {
                    tokens_used += mem.metadata.tokens;
                    loaded.insert(key);
                    debug!(
                        "Bootstrap: loaded active/{} ({:?}, {} tokens)",
                        entry.filename, entry.frequency, mem.metadata.tokens
                    );
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(Scenario::Active, &entry.filename, &e)
                        .await
                    {
                        warn!("Bootstrap: failed to load active/{}: {}", entry.filename, e);
                    }
                }
            }
        }

        Ok(memories)
    }

    /// Phase 2: Load scenario-specific hot/warm items.
    ///
    /// Uses SQLite metadata queries instead of filesystem scanning.
    /// 1. Load hot items first (regardless of tags)
    /// 2. Load warm items matching query tags
    /// 3. Stop when budget exceeded
    async fn load_scenario(
        &self,
        scenario: Scenario,
        tags: &[String],
        loaded: &mut HashSet<String>,
    ) -> Result<Vec<MemoryFile>> {
        let mut memories = Vec::new();
        let budget = self.budget.scenario;

        // Skip profile and active (already loaded in bootstrap)
        if matches!(scenario, Scenario::Profile | Scenario::Active) {
            return Ok(memories);
        }

        // Query from SQLite — no filesystem scanning
        let entries = self
            .metadata_store
            .query_entries(scenario)
            .await
            .unwrap_or_default();

        let mut tokens_used = 0;

        // Phase 2a: Load hot items first
        for entry in entries.iter().filter(|e| e.frequency == Frequency::Hot) {
            let key = format!("{}/{}", scenario.dir_name(), entry.filename);
            if loaded.contains(&key) {
                continue;
            }

            if tokens_used + entry.tokens as usize > budget {
                debug!("Scenario: budget exhausted at hot item {}", entry.title);
                break;
            }

            match self.store.read(scenario, &entry.filename).await {
                Ok(mem) => {
                    debug!("Scenario: loaded hot item {}", entry.title);
                    tokens_used += entry.tokens as usize;
                    loaded.insert(key);
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(scenario, &entry.filename, &e)
                        .await
                    {
                        warn!("Scenario: failed to load hot item {}: {}", entry.title, e);
                    }
                }
            }
        }

        // Phase 2b: Load warm items matching tags
        for entry in entries.iter().filter(|e| e.frequency == Frequency::Warm) {
            let key = format!("{}/{}", scenario.dir_name(), entry.filename);
            if loaded.contains(&key) {
                continue;
            }

            // Check tag match
            if !tags.is_empty() {
                let matches = entry
                    .tags
                    .iter()
                    .any(|t| tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)));
                if !matches {
                    continue;
                }
            }

            if tokens_used + entry.tokens as usize > budget {
                debug!("Scenario: budget exhausted at warm item {}", entry.title);
                break;
            }

            match self.store.read(scenario, &entry.filename).await {
                Ok(mem) => {
                    debug!("Scenario: loaded warm item {}", entry.title);
                    tokens_used += entry.tokens as usize;
                    loaded.insert(key);
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(scenario, &entry.filename, &e)
                        .await
                    {
                        warn!("Scenario: failed to load warm item {}: {}", entry.title, e);
                    }
                }
            }
        }

        Ok(memories)
    }

    /// Phase 3: On-demand search via retrieval engine.
    ///
    /// Uses RetrievalEngine.search() for tag+embedding combined results,
    /// then loads .md files until budget exhausted. Skips already-loaded files.
    async fn load_on_demand(
        &self,
        query: &MemoryQuery,
        budget: usize,
        loaded: &mut HashSet<String>,
    ) -> Result<Vec<MemoryFile>> {
        let mut memories = Vec::new();
        let mut tokens_used = 0;

        // Search via retrieval engine
        let results = match self.retrieval.search(query).await {
            Ok(r) => r,
            Err(e) => {
                warn!("On-demand: search failed: {}", e);
                return Ok(memories);
            }
        };

        debug!("On-demand: {} search results", results.len());

        for result in results {
            // Skip profile and active (already in bootstrap)
            if matches!(result.scenario, Scenario::Profile | Scenario::Active) {
                continue;
            }

            let key = format!("{}/{}", result.scenario.dir_name(), result.memory_path);
            if loaded.contains(&key) {
                debug!("On-demand: skipping already loaded {}", key);
                continue;
            }

            let tokens = result.tokens as usize;
            if tokens_used + tokens > budget {
                debug!("On-demand: budget exhausted at {}", result.title);
                break;
            }

            match self.store.read(result.scenario, &result.memory_path).await {
                Ok(mem) => {
                    debug!("On-demand: loaded {}", result.title);
                    tokens_used += tokens;
                    loaded.insert(key);
                    memories.push(mem);
                }
                Err(e) => {
                    if !self
                        .cleanup_stale_if_not_found(result.scenario, &result.memory_path, &e)
                        .await
                    {
                        warn!("On-demand: failed to load {}: {}", result.title, e);
                    }
                }
            }
        }

        Ok(memories)
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
    /// Breakdown by phase.
    pub phase_breakdown: PhaseBreakdown,
}

/// Token breakdown by loading phase.
#[derive(Debug)]
pub struct PhaseBreakdown {
    pub bootstrap_tokens: usize,
    pub scenario_tokens: usize,
    pub on_demand_tokens: usize,
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
        let file_content = serialize_memory_file(&meta, &content);

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

        let manager = MemoryManager::new(base_dir, &pool).await.unwrap();
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
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 700);
        assert_eq!(context.phase_breakdown.scenario_tokens, 0);
        assert_eq!(context.phase_breakdown.on_demand_tokens, 0);
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
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 250);
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

        // Should load hot1 (800) + hot2 (800) = 1600, but cap at 1500
        // Actually hot1 (800) fits, hot2 would exceed 1500, so only hot1 loads
        assert!(context.phase_breakdown.scenario_tokens <= 1500);
        assert!(context.memories.iter().any(|m| m.metadata.title == "Hot 1"));
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

        // Should load cold files in on-demand phase
        assert!(context.phase_breakdown.on_demand_tokens > 0);
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
        let custom_manager = MemoryManager::with_components(
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
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
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 100);
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

/// Implement MemoryProvider trait for MemoryManager.
///
/// This allows HistoryCoordinator to depend on the trait rather than
/// the concrete type, enabling test doubles and future alternative backends.
#[async_trait::async_trait]
impl crate::agent::memory_provider::MemoryProvider for MemoryManager {
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
