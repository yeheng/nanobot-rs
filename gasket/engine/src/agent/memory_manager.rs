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

use anyhow::Result;
use gasket_storage::memory::*;
use gasket_storage::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{debug, warn};

/// Facade managing three-phase memory loading for the agent loop.
pub struct MemoryManager {
    store: FileMemoryStore,
    index_manager: FileIndexManager,
    metadata_store: MetadataStore,
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
        let retrieval = RetrievalEngine::new(MetadataStore::new(pool.clone()), embedding_store);
        let budget = TokenBudget::default();

        Ok(Self {
            store,
            index_manager,
            metadata_store,
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
        let retrieval = RetrievalEngine::new(MetadataStore::new(pool.clone()), embedding_store);

        Ok(Self {
            store,
            index_manager,
            metadata_store,
            retrieval,
            budget,
        })
    }

    /// Create a MemoryManager with pre-built components (for testing).
    pub fn with_components(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        metadata_store: MetadataStore,
        retrieval: RetrievalEngine,
        budget: TokenBudget,
    ) -> Self {
        Self {
            store,
            index_manager,
            metadata_store,
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

        // Enforce hard cap
        if total_used > self.budget.total_cap {
            warn!(
                "Total tokens {} exceeds cap {}, truncating",
                total_used, self.budget.total_cap
            );
            let mut truncated = Vec::new();
            let mut accum = 0;
            for mem in loaded {
                // Check if adding this memory would exceed the cap
                if accum + mem.metadata.tokens > self.budget.total_cap {
                    break;
                }
                accum += mem.metadata.tokens;
                truncated.push(mem);
            }
            return Ok(MemoryContext {
                memories: truncated,
                tokens_used: accum,
                phase_breakdown: PhaseBreakdown {
                    bootstrap_tokens,
                    scenario_tokens,
                    on_demand_tokens,
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
                    warn!(
                        "Bootstrap: failed to load profile/{}: {}",
                        entry.filename, e
                    );
                }
            }
        }

        // Load active entries from SQLite (already sorted: Hot → Warm → Cold)
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
                    warn!("Bootstrap: failed to load active/{}: {}", entry.filename, e);
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
                    warn!("Scenario: failed to load hot item {}: {}", entry.title, e);
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
                    warn!("Scenario: failed to load warm item {}: {}", entry.title, e);
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
                    warn!("On-demand: failed to load {}: {}", result.title, e);
                }
            }
        }

        Ok(memories)
    }
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
            embedding_store,
        );
        let custom_manager =
            MemoryManager::with_components(store, index_manager, metadata_store, retrieval, budget);

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
        // MemoryManager doesn't expose a public search() method directly.
        // Use load_for_context with text-based MemoryQuery as a workaround.
        // Phase 2+ will expose a proper search method via RetrievalEngine.
        let memory_query = MemoryQuery {
            text: Some(query.to_string()),
            tags: vec![],
            scenario: None,
            max_tokens: Some(top_k * 200),
        };
        let ctx = self.load_for_context(&memory_query).await?;
        // Convert MemoryFile hits to MemoryHit
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
}
