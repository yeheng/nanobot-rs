//! Memory manager facade with three-phase loading for context injection.
//!
//! The MemoryManager orchestrates the storage-layer components (FileMemoryStore,
//! FileIndexManager, RetrievalEngine) to provide three-phase memory loading:
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
    retrieval: RetrievalEngine,
    budget: TokenBudget,
}

impl MemoryManager {
    /// Create a new MemoryManager with default components.
    ///
    /// # Arguments
    /// * `base_dir` - Memory base directory (e.g., ~/.gasket/memory/)
    /// * `pool` - SQLite connection pool for EmbeddingStore
    pub async fn new(base_dir: PathBuf, pool: &SqlitePool) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
        // Create a separate index manager instance for retrieval engine
        let retrieval_index = FileIndexManager::new(base_dir.clone());
        let retrieval = RetrievalEngine::new(retrieval_index, embedding_store);
        let budget = TokenBudget::default();

        Ok(Self {
            store,
            index_manager,
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
        // Create a separate index manager instance for retrieval engine
        let retrieval_index = FileIndexManager::new(base_dir.clone());
        let retrieval = RetrievalEngine::new(retrieval_index, embedding_store);

        Ok(Self {
            store,
            index_manager,
            retrieval,
            budget,
        })
    }

    /// Create a MemoryManager with pre-built components (for testing).
    pub fn with_components(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        retrieval: RetrievalEngine,
        budget: TokenBudget,
    ) -> Self {
        Self {
            store,
            index_manager,
            retrieval,
            budget,
        }
    }

    /// Initialize memory system (create directories, etc.).
    pub async fn init(&self) -> Result<()> {
        self.store.init().await
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
        let mut bootstrap_tokens = 0;
        let mut scenario_tokens = 0;
        let mut on_demand_tokens = 0;

        // Phase 1: Bootstrap (always loaded)
        debug!("Phase 1: Loading bootstrap memories");
        let bootstrap_memories = self.load_bootstrap(&mut loaded_filenames).await?;
        bootstrap_tokens = bootstrap_memories.iter().map(|m| m.metadata.tokens).sum();
        loaded.extend(bootstrap_memories);
        debug!("Phase 1 complete: {} tokens", bootstrap_tokens);

        // Phase 2: Scenario-specific
        let scenario = query.scenario.unwrap_or(Scenario::Knowledge);
        debug!("Phase 2: Loading scenario memories for {:?}", scenario);
        let scenario_memories = self
            .load_scenario(scenario, &query.tags, &mut loaded_filenames)
            .await?;
        scenario_tokens = scenario_memories.iter().map(|m| m.metadata.tokens).sum();
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
    /// Strategy:
    /// - All .md files in profile/ directory (profile is always high-priority)
    /// - Active files with Hot frequency first, then Warm if budget allows
    /// - Skips Cold and Archived items
    async fn load_bootstrap(&self, loaded: &mut HashSet<String>) -> Result<Vec<MemoryFile>> {
        let mut memories = Vec::new();
        let budget = self.budget.bootstrap;
        let mut tokens_used = 0;

        // Load all profile files (profile data is always loaded)
        if let Ok(profile_files) = self.store.list(Scenario::Profile).await {
            for filename in profile_files {
                if !loaded.insert(format!("profile/{}", filename)) {
                    continue; // Already loaded
                }
                match self.store.read(Scenario::Profile, &filename).await {
                    Ok(mem) => {
                        tokens_used += mem.metadata.tokens;
                        debug!(
                            "Bootstrap: loaded profile/{} ({} tokens)",
                            filename, mem.metadata.tokens
                        );
                        memories.push(mem);
                    }
                    Err(e) => {
                        warn!("Bootstrap: failed to load profile/{}: {}", filename, e);
                    }
                }
            }
        }

        // Load active files by frequency priority (Hot first, then Warm)
        if let Ok(active_files) = self.store.list(Scenario::Active).await {
            // Collect metadata for priority-based loading
            let mut hot_items = Vec::new();
            let mut warm_items = Vec::new();

            for filename in active_files {
                let key = format!("active/{}", filename);
                if loaded.contains(&key) {
                    continue;
                }
                // Read frontmatter to determine frequency
                match self.store.read(Scenario::Active, &filename).await {
                    Ok(mem) => match mem.metadata.frequency {
                        Frequency::Hot => hot_items.push((filename, mem)),
                        Frequency::Warm => warm_items.push((filename, mem)),
                        Frequency::Cold | Frequency::Archived => {
                            debug!(
                                "Bootstrap: skipping active/{} (frequency: {:?})",
                                filename, mem.metadata.frequency
                            );
                        }
                    },
                    Err(e) => {
                        warn!("Bootstrap: failed to read active/{}: {}", filename, e);
                    }
                }
            }

            // Load Hot items first
            for (filename, mem) in hot_items {
                let key = format!("active/{}", filename);
                if tokens_used + mem.metadata.tokens > budget {
                    debug!(
                        "Bootstrap: budget exhausted at hot item active/{}",
                        filename
                    );
                    break;
                }
                tokens_used += mem.metadata.tokens;
                loaded.insert(key);
                debug!(
                    "Bootstrap: loaded hot active/{} ({} tokens)",
                    filename, mem.metadata.tokens
                );
                memories.push(mem);
            }

            // Then load Warm items if budget allows
            for (filename, mem) in warm_items {
                let key = format!("active/{}", filename);
                if tokens_used + mem.metadata.tokens > budget {
                    debug!(
                        "Bootstrap: budget exhausted at warm item active/{}",
                        filename
                    );
                    break;
                }
                tokens_used += mem.metadata.tokens;
                loaded.insert(key);
                debug!(
                    "Bootstrap: loaded warm active/{} ({} tokens)",
                    filename, mem.metadata.tokens
                );
                memories.push(mem);
            }
        }

        Ok(memories)
    }

    /// Phase 2: Load scenario-specific hot/warm items.
    ///
    /// Strategy:
    /// 1. Scan .md files directly for fresh metadata (no _INDEX.md dependency)
    /// 2. Load hot items first (regardless of tags)
    /// 3. Load warm items matching query tags
    /// 4. Stop when budget exceeded
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

        // Scan files directly — no _INDEX.md dependency
        let entries = match self.index_manager.scan_entries(scenario).await {
            Ok(e) => e,
            Err(e) => {
                warn!("Scenario: failed to scan {:?}: {}", scenario, e);
                return Ok(memories);
            }
        };

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
        manager
            .index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

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

        // Regenerate index
        manager
            .index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

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

        // Regenerate index
        manager
            .index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

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

        // Regenerate index
        manager
            .index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

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

        manager
            .index_manager
            .regenerate(Scenario::Profile)
            .await
            .unwrap();

        // Create manager with custom budget
        let store = FileMemoryStore::new(memory_dir.clone());
        let index_manager = FileIndexManager::new(memory_dir.clone());
        let pool = SqliteStore::with_path(temp_dir.path().join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let embedding_store = EmbeddingStore::new(pool);
        // Create a separate index manager instance for retrieval engine
        let retrieval_index = FileIndexManager::new(memory_dir.clone());
        let retrieval = RetrievalEngine::new(retrieval_index, embedding_store);
        let custom_manager =
            MemoryManager::with_components(store, index_manager, retrieval, budget);

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
        manager
            .index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

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
