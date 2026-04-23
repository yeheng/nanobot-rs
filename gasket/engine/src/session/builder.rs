//! Session builder — composable construction of AgentSession services.
//!
//! Replaces the monolithic `with_services` constructor with a clean builder.
//! All intermediate services are constructed inside `build()` as local variables —
//! no partial initialization, no `Option` fields, no `expect()` panics.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::error::AgentError;
use crate::hooks::HookRegistry;
use crate::kernel::RuntimeContext;
use crate::session::compactor::ContextCompactor;
use crate::session::config::AgentConfigExt;
use crate::session::context::AgentContext;
use crate::session::plugins::WikiLifecyclePlugin;
use crate::session::{prompt, AgentConfig, AgentSession, WikiComponents};
use crate::wiki::{PageIndex, PageStore, WikiLog};
use gasket_providers::LlmProvider;
use gasket_storage::{EventStore, SqliteStore};

/// Builder for constructing an `AgentSession`.
///
/// Holds only the external inputs; all internal services are built locally
/// inside `build()` in the correct dependency order.
pub struct SessionBuilder {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    memory_store: Arc<crate::session::MemoryStore>,
}

impl SessionBuilder {
    /// Start building a session with required dependencies.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<crate::tools::ToolRegistry>,
        memory_store: Arc<crate::session::MemoryStore>,
    ) -> Self {
        Self {
            provider,
            workspace,
            config,
            tools,
            memory_store,
        }
    }

    /// Build the complete `AgentSession`.
    ///
    /// All services are constructed in dependency order as local variables —
    /// the compiler guarantees every value is initialized before use.
    pub async fn build(self) -> Result<AgentSession, AgentError> {
        // ── 1. Storage layer ─────────────────────────────────────────
        let sqlite_store = Arc::new(self.memory_store.sqlite_store().clone());
        let event_store = Arc::new(EventStore::new(self.memory_store.sqlite_store().pool()));

        // ── 2. Kernel runtime context ────────────────────────────────
        let kernel_config = self.config.to_kernel_config();
        let runtime_ctx = RuntimeContext {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: kernel_config,
            spawner: None,
            token_tracker: None,
            checkpoint_callback: std::sync::Arc::new(crate::kernel::NoopCheckpoint),
        };

        // ── 3. Agent context ─────────────────────────────────────────
        let context = AgentContext::persistent(event_store.clone(), sqlite_store.clone());

        // ── 4. Context compactor ─────────────────────────────────────
        let history_config = gasket_storage::HistoryConfig {
            max_events: self.config.memory_window,
            ..Default::default()
        };
        let mut compactor = ContextCompactor::new(
            self.provider.clone(),
            event_store.clone(),
            sqlite_store.clone(),
            self.config.model.clone(),
            history_config.token_budget,
        );
        if let Some(ref prompt_text) = self.config.summarization_prompt {
            compactor = compactor.with_summarization_prompt(prompt_text.clone());
        }
        compactor = compactor
            .with_checkpoint_config(crate::session::compactor::CheckpointConfig::default());
        let compactor = Some(Arc::new(compactor));

        // ── 5. System prompt and skills (merged) ─────────────────────
        let mut system_prompt =
            match prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_FULL).await {
                Ok(sp) => sp,
                Err(e) => {
                    warn!("Failed to load system prompt: {}", e);
                    String::new()
                }
            };
        if let Some(skills) = prompt::load_skills_context(&self.workspace).await {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&skills);
        }

        // ── 6. Lifecycle plugins (wiki, cost tracking, etc.) ─────────
        let mut plugins: Vec<Arc<dyn crate::session::SessionLifecyclePlugin>> = Vec::new();

        // Wiki plugin
        if let Some(wiki_components) = build_wiki_components(&sqlite_store, &self.config).await {
            plugins.push(Arc::new(WikiLifecyclePlugin::new(
                wiki_components.page_store.clone(),
                wiki_components.page_index.clone(),
                wiki_components.wiki_log.clone(),
            )));
        }

        // ── 7. Hook registry ─────────────────────────────────────────
        let hooks = build_hooks_registry();

        let pending_done = tokio_util::task::TaskTracker::new();

        Ok(AgentSession {
            runtime_ctx,
            context,
            config: self.config,
            system_prompt,
            hooks,
            compactor,
            plugins,
            pending_done,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — pure functions, no builder state
// ---------------------------------------------------------------------------

/// Build wiki components (optional).
///
/// Returns `None` when wiki is disabled by config or initialization failed.
async fn build_wiki_components(
    sqlite_store: &Arc<SqliteStore>,
    config: &AgentConfig,
) -> Option<WikiComponents> {
    let wiki_config = match config.wiki.as_ref() {
        Some(cfg) if cfg.enabled => cfg.clone(),
        _ => return None,
    };

    let pool = sqlite_store.pool().clone();
    let wiki_base = PathBuf::from(&wiki_config.base_path);

    let store = PageStore::new(pool.clone(), wiki_base.clone());
    if let Err(e) = store.init_dirs().await {
        warn!("Failed to init wiki PageStore: {}", e);
        return None;
    }
    let store = Arc::new(store);

    let tantivy_dir = wiki_base.join(".tantivy");
    let index = match PageIndex::open(tantivy_dir) {
        Ok(idx) => Arc::new(idx),
        Err(e) => {
            warn!("Failed to open Tantivy index: {}", e);
            return None;
        }
    };

    let log = Arc::new(WikiLog::new(pool));

    if let Err(e) = gasket_storage::wiki::tables::create_wiki_tables(&sqlite_store.pool()).await {
        warn!("Failed to create wiki tables: {}", e);
        return None;
    }

    // Repair any Tantivy index drift caused by unclean shutdown.
    match store.repair_index(&index).await {
        Ok(0) => {}
        Ok(n) => info!("Repaired {} pages in Tantivy index after startup", n),
        Err(e) => warn!("Tantivy index repair failed: {}", e),
    }

    info!(
        "Wiki knowledge system initialized at {}",
        wiki_config.base_path
    );

    Some(WikiComponents {
        page_store: store,
        page_index: index,
        wiki_log: log,
    })
}

/// Build the hook registry.
fn build_hooks_registry() -> Arc<HookRegistry> {
    crate::session::history::builder::build_default_hooks_builder().build_shared()
}

/// Build an AgentSession with all services initialized.
pub async fn build_session(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    memory_store: Arc<crate::session::MemoryStore>,
) -> Result<AgentSession, AgentError> {
    SessionBuilder::new(provider, workspace, config, tools, memory_store)
        .build()
        .await
}
