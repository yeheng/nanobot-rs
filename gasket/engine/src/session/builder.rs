//! Session builder — composable construction of AgentSession services.
//!
//! Replaces the monolithic `with_services` constructor with a clean builder.
//! All intermediate services are constructed inside `build()` as local variables —
//! no partial initialization, no `Option` fields, no `expect()` panics.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::error::AgentError;
use crate::kernel::RuntimeContext;
use crate::session::compactor::ContextCompactor;
use crate::session::config::AgentConfigExt;
use crate::session::context::AgentContext;

use crate::session::finalizer::ResponseFinalizer;
use crate::session::{prompt, AgentConfig, AgentSession, WikiComponents};
use crate::wiki::{PageIndex, PageStore, WikiLog};
use gasket_storage::wiki::TantivyPageIndex;
use gasket_providers::LlmProvider;
use gasket_storage::{EventStore, SessionStore};

/// Builder for constructing an `AgentSession`.
///
/// Holds only the external inputs; all internal services are built locally
/// inside `build()` in the correct dependency order.
pub struct SessionBuilder {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    sqlite_store: Arc<gasket_storage::SqliteStore>,
}

impl SessionBuilder {
    /// Start building a session with required dependencies.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<crate::tools::ToolRegistry>,
        sqlite_store: Arc<gasket_storage::SqliteStore>,
    ) -> Self {
        Self {
            provider,
            workspace,
            config,
            tools,
            sqlite_store,
        }
    }

    /// Build the complete `AgentSession`.
    ///
    /// All services are constructed in dependency order as local variables —
    /// the compiler guarantees every value is initialized before use.
    pub async fn build(self) -> Result<AgentSession, AgentError> {
        // ── 1. Storage layer ─────────────────────────────────────────
        let pool = self.sqlite_store.pool();
        let session_store = Arc::new(SessionStore::new(pool.clone()));
        let event_store = Arc::new(EventStore::new(pool));

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
        let context = AgentContext::persistent(event_store.clone(), session_store.clone());

        // ── 5. Context compactor ─────────────────────────────────────
        let history_config = gasket_storage::HistoryConfig {
            max_events: self.config.memory_window,
            ..Default::default()
        };
        let mut compactor = ContextCompactor::new(
            self.provider.clone(),
            event_store.clone(),
            session_store.clone(),
            self.config.model.clone(),
            history_config.token_budget,
        );
        if let Some(ref prompt_text) = self.config.prompts.summarization {
            compactor = compactor.with_summarization_prompt(prompt_text.clone());
        }
        let mut checkpoint_config = crate::session::compactor::CheckpointConfig::default();
        if let Some(ref prompt_text) = self.config.prompts.checkpoint {
            checkpoint_config.prompt = prompt_text.clone();
        }
        compactor = compactor.with_checkpoint_config(checkpoint_config);
        let compactor = Some(Arc::new(compactor));

        // ── 6. System prompt and skills (merged) ─────────────────────
        let mut system_prompt = match prompt::load_system_prompt(
            &self.workspace,
            prompt::BOOTSTRAP_FILES_FULL,
            self.config.prompts.identity_prefix.as_deref(),
        )
        .await
        {
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

        // ── 7. Wiki components (optional) ──────────────────────────
        let wiki_components = build_wiki_components(&self.sqlite_store, &self.config).await;

        // ── 8. Hook registry ─────────────────────────────────────────
        let hooks_builder = crate::session::history::builder::build_default_hooks_builder();

        let hooks = hooks_builder.build_shared();

        let pending_done = tokio_util::task::TaskTracker::new();

        let finalizer = ResponseFinalizer::new(hooks.clone(), compactor.clone(), None, self.config.max_tokens);

        Ok(AgentSession {
            runtime_ctx,
            context,
            config: self.config,
            system_prompt,
            hooks,
            compactor,
            wiki_components,
            pricing: None,
            finalizer,
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
    sqlite_store: &gasket_storage::SqliteStore,
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
    let tantivy_index = match TantivyPageIndex::open(tantivy_dir) {
        Ok(idx) => Arc::new(idx),
        Err(e) => {
            warn!("Failed to open Tantivy index: {}", e);
            return None;
        }
    };
    let index = Arc::new(PageIndex::new(tantivy_index));

    let log = Arc::new(WikiLog::new(pool));

    if let Err(e) = gasket_storage::wiki::tables::create_wiki_tables(&sqlite_store.pool()).await {
        warn!("Failed to create wiki tables: {}", e);
        return None;
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

/// Build an AgentSession with all services initialized.
pub async fn build_session(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    sqlite_store: Arc<gasket_storage::SqliteStore>,
) -> Result<AgentSession, AgentError> {
    SessionBuilder::new(provider, workspace, config, tools, sqlite_store)
        .build()
        .await
}
