//! Memory manager facade — thin delegation to writer + loader.
//!
//! The `MemoryManager` composes `MemoryWriter` (CRUD + sync) and
//! `MemoryLoader` (three-phase loading + search) behind a single public type.
//! Callers see the same API as before; the internal split is an
//! implementation detail that keeps each concern under 300 lines.

use anyhow::Result;
use gasket_storage::memory::*;
use gasket_storage::SqlitePool;
use std::path::PathBuf;
use tracing::{debug, info};

mod access;
mod loader;
mod types;
mod writer;

pub use types::{MemoryContext, PhaseBreakdown, ReindexReport};

use access::AccessTracker;
use loader::MemoryLoader;
use writer::MemoryWriter;

// ── Facade ────────────────────────────────────────────────────────────────

/// Facade orchestrating memory read/write through two sub-engines.
///
/// - `writer` — CRUD, sync, reindex (filesystem SSOT + SQLite write-through)
/// - `loader` — three-phase context loading, semantic search, access tracking
pub struct MemoryManager {
    writer: MemoryWriter,
    loader: MemoryLoader,
}

impl MemoryManager {
    /// Create a new MemoryManager.
    pub async fn new(
        base_dir: PathBuf,
        pool: &SqlitePool,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self> {
        let store = FileMemoryStore::new(base_dir.clone());
        let index_manager = FileIndexManager::new(base_dir.clone());
        let metadata_store = MetadataStore::new(pool.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());

        let embedder_for_retrieval = embedder.clone_box();
        let retrieval = RetrievalEngine::new(
            MetadataStore::new(pool.clone()),
            EmbeddingStore::new(pool.clone()),
            Some(embedder_for_retrieval),
        );

        let budget = TokenBudget::default();
        let access = AccessTracker::new(MetadataStore::new(pool.clone()));

        let writer = MemoryWriter::new(
            store,
            index_manager,
            metadata_store.clone(),
            embedding_store,
            embedder,
        );

        let loader = MemoryLoader::new(
            FileMemoryStore::new(base_dir),
            metadata_store,
            EmbeddingStore::new(pool.clone()),
            retrieval,
            budget,
            access,
        );

        Ok(Self { writer, loader })
    }

    /// Override the token budget (builder-style).
    pub fn with_budget(mut self, budget: Option<TokenBudget>) -> Self {
        if let Some(b) = budget {
            self.loader = self.loader.with_budget(b);
        }
        self
    }

    /// Create a MemoryManager with pre-built components (for testing).
    #[allow(clippy::too_many_arguments)]
    pub fn with_components(
        store: FileMemoryStore,
        index_manager: FileIndexManager,
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        retrieval: RetrievalEngine,
        embedder: Box<dyn Embedder>,
        budget: TokenBudget,
        pool: SqlitePool,
    ) -> Self {
        let access = AccessTracker::new(metadata_store.clone());

        let writer = MemoryWriter::new(
            store.clone(),
            index_manager,
            metadata_store.clone(),
            embedding_store,
            embedder,
        );

        let loader = MemoryLoader::new(
            store,
            metadata_store,
            EmbeddingStore::new(pool),
            retrieval,
            budget,
            access,
        );

        Self { writer, loader }
    }

    /// Initialize memory system (create directories, sync metadata to SQLite).
    pub async fn init(&self) -> Result<()> {
        self.writer.store().init().await?;
        self.writer.sync_all().await?;
        info!("Memory manager initialized");
        Ok(())
    }

    // ── Thin delegation ──────────────────────────────────────────────────

    /// Sync filesystem metadata into SQLite for all scenarios.
    pub async fn sync_all(&self) -> Result<()> {
        self.writer.sync_all().await
    }

    /// Create a new memory file (write-through).
    pub async fn create_memory(
        &self,
        scenario: Scenario,
        title: &str,
        tags: &[String],
        frequency: Frequency,
        content: &str,
    ) -> Result<String> {
        self.writer
            .create_memory(scenario, title, tags, frequency, content)
            .await
    }

    /// Update an existing memory file (write-through).
    pub async fn update_memory(
        &self,
        scenario: Scenario,
        filename: &str,
        content: &str,
    ) -> Result<()> {
        self.writer.update_memory(scenario, filename, content).await
    }

    /// Delete a memory file (write-through).
    pub async fn delete_memory(&self, scenario: Scenario, filename: &str) -> Result<()> {
        self.writer.delete_memory(scenario, filename).await
    }

    /// Full reindex (destructive, idempotent).
    pub async fn reindex(&self) -> Result<ReindexReport> {
        self.writer.reindex().await
    }

    /// Three-phase memory loading for context injection.
    pub async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext> {
        let ctx = self.loader.load_for_context(query).await?;
        debug!(
            "Memory context loaded: {} memories, {} tokens (bootstrap={}, scenario={}, on_demand={})",
            ctx.memories.len(),
            ctx.tokens_used,
            ctx.phase_breakdown.bootstrap_tokens,
            ctx.phase_breakdown.scenario_tokens,
            ctx.phase_breakdown.on_demand_tokens,
        );
        Ok(ctx)
    }

    /// Semantic search with real relevance scores.
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>> {
        let results = self.loader.search(query, top_k).await?;
        debug!(
            "Memory search returned {} results (top_k={})",
            results.len(),
            top_k
        );
        Ok(results)
    }

    /// Flush access log entries on graceful shutdown.
    pub async fn shutdown_flush(&self) -> Result<()> {
        self.loader.shutdown().await
    }

    /// Access the metadata store (for tests).
    #[cfg(test)]
    pub(crate) fn metadata_store(&self) -> &MetadataStore {
        self.writer.metadata_store()
    }
}

// ── MemoryProvider trait impl ─────────────────────────────────────────────

#[async_trait::async_trait]
impl super::store::MemoryProvider for MemoryManager {
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext> {
        self.load_for_context(query).await
    }

    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>> {
        self.search(query, top_k).await
    }

    async fn create_memory(
        &self,
        scenario: Scenario,
        _filename: &str,
        title: &str,
        tags: &[String],
        frequency: Frequency,
        content: &str,
    ) -> Result<()> {
        self.create_memory(scenario, title, tags, frequency, content)
            .await?;
        Ok(())
    }

    async fn update_memory(&self, scenario: Scenario, filename: &str, content: &str) -> Result<()> {
        self.update_memory(scenario, filename, content).await
    }

    async fn delete_memory(&self, scenario: Scenario, filename: &str) -> Result<()> {
        self.delete_memory(scenario, filename).await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

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

        let db_path = temp_dir.path().join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        let embedder: Box<dyn Embedder> = Box::new(NoopEmbedder::new(384));
        let manager = MemoryManager::new(base_dir, &pool, embedder).await.unwrap();
        manager.init().await.unwrap();

        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_bootstrap_loads_by_frequency_not_filename() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

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

        manager.sync_all().await.unwrap();

        let query = MemoryQuery::new();
        let context = manager.load_for_context(&query).await.unwrap();

        assert_eq!(context.memories.len(), 4);
        assert_eq!(context.tokens_used, 700);
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 700);
        assert_eq!(context.phase_breakdown.scenario_tokens, 0);
        assert_eq!(context.phase_breakdown.on_demand_tokens, 0);

        assert!(!context
            .memories
            .iter()
            .any(|m| m.metadata.title == "Old Idea"));
    }

    #[tokio::test]
    async fn test_bootstrap_loads_arbitrary_named_hot_files() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        create_memory_file(
            &memory_dir,
            Scenario::Active,
            "todo.md",
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
            "sprint_plan.md",
            "Sprint Plan",
            &["active"],
            Frequency::Warm,
            150,
        )
        .await
        .unwrap();

        manager.sync_all().await.unwrap();

        let query = MemoryQuery::new();
        let context = manager.load_for_context(&query).await.unwrap();

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
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 250);
    }

    #[tokio::test]
    async fn test_scenario_phase_respects_budget() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        manager.sync_all().await.unwrap();

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

        manager.sync_all().await.unwrap();

        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_tag("rust");
        let context = manager.load_for_context(&query).await.unwrap();

        assert!(context.tokens_used <= 4000);
        assert!(context.memories.iter().any(|m| m.metadata.title == "Hot 1"));
        assert!(context.memories.iter().any(|m| m.metadata.title == "Hot 2"));
        assert_eq!(context.phase_breakdown.scenario_tokens, 800);
        assert_eq!(context.phase_breakdown.on_demand_tokens, 800);
    }

    #[tokio::test]
    async fn test_on_demand_fills_remaining() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

        manager.sync_all().await.unwrap();

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

        manager.sync_all().await.unwrap();

        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_text("search query")
            .with_tag("search");
        let context = manager.load_for_context(&query).await.unwrap();

        assert!(
            context.tokens_used > 0,
            "On-demand phase should load cold items from tag search"
        );
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 0);
        assert_eq!(context.phase_breakdown.scenario_tokens, 0);
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

        let store = FileMemoryStore::new(memory_dir.clone());
        let index_manager = FileIndexManager::new(memory_dir.clone());
        let pool = SqliteStore::with_path(temp_dir.path().join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let metadata_store = MetadataStore::new(pool.clone());
        let embedding_store = EmbeddingStore::new(pool.clone());
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
            None,
        );
        let embedder: Box<dyn Embedder> = Box::new(NoopEmbedder::new(384));
        let custom_manager = MemoryManager::with_components(
            store,
            index_manager,
            metadata_store,
            embedding_store,
            retrieval,
            embedder,
            budget,
            pool,
        );

        let query = MemoryQuery::new();
        let context = custom_manager.load_for_context(&query).await.unwrap();

        assert!(context.tokens_used <= 1000);
        assert_eq!(context.phase_breakdown.bootstrap_tokens, 0);
        assert_eq!(context.phase_breakdown.scenario_tokens, 0);
        assert_eq!(context.phase_breakdown.on_demand_tokens, 0);
    }

    #[tokio::test]
    async fn test_graceful_skip_on_corrupted_file() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

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

        let corrupted_path = memory_dir.join("profile/corrupted.md");
        tokio::fs::write(&corrupted_path, "invalid frontmatter\n---\ncontent")
            .await
            .unwrap();

        manager.sync_all().await.unwrap();

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

        let query = MemoryQuery::new()
            .with_scenario(Scenario::Knowledge)
            .with_text("duplicate")
            .with_tag("test");
        let context = manager.load_for_context(&query).await.unwrap();

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

        assert!(!filename.is_empty());

        let entries = manager
            .metadata_store()
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

        let entries_before = manager
            .metadata_store()
            .query_entries(Scenario::Knowledge)
            .await
            .unwrap();
        assert_eq!(1, entries_before.len());

        manager
            .delete_memory(Scenario::Knowledge, "to_delete.md")
            .await
            .unwrap();

        let entries_after = manager
            .metadata_store()
            .query_entries(Scenario::Knowledge)
            .await
            .unwrap();
        assert_eq!(0, entries_after.len());
    }

    #[tokio::test]
    async fn test_reindex() {
        let (manager, temp_dir) = setup_manager().await;
        let memory_dir = temp_dir.path().join("memory");

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
