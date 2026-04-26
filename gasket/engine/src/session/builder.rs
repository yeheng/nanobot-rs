//! Session builder — composable construction of AgentSession services.
//!
//! Replaces the monolithic `with_services` constructor with a clean builder.
//! All intermediate services are constructed inside `build()` as local variables —
//! no partial initialization, no `Option` fields, no `expect()` panics.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::warn;

use crate::error::AgentError;
use crate::kernel::RuntimeContext;
use crate::session::compactor::ContextCompactor;
use crate::session::config::AgentConfigExt;

use crate::session::finalizer::ResponseFinalizer;
use crate::session::{prompt, AgentConfig, AgentSession};
use gasket_providers::LlmProvider;
use gasket_storage::{EventStore, SessionStore};

/// Wiki preparation prompt appended to system prompt when wiki is enabled.
///
/// Instructs the agent to proactively query wiki via tools before responding,
/// replacing the old automatic context injection mechanism.
const WIKI_PREPARATION_PROMPT: &str = "\
## Wiki Knowledge System

You have access to a personal wiki knowledge base via these tools:
- `wiki_search(query)`: Search wiki pages by keyword
- `wiki_read(path)`: Read a specific wiki page by path
- `wiki_write(path, title, content)`: Create or update a wiki page

**Preparation Protocol**: Before responding to any user query, always use `wiki_search` \
to check if relevant knowledge already exists in the wiki. This ensures your responses \
build upon accumulated knowledge rather than starting from scratch.";

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

        // ── 3. Context compactor ─────────────────────────────────────
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

        // ── 7. Wiki availability check (prompt only) ──────────────
        if is_wiki_available(&self.config) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(WIKI_PREPARATION_PROMPT);
        }

        // ── 8. Hook registry ─────────────────────────────────────────
        let hooks_builder = crate::session::history::builder::build_default_hooks_builder();

        let hooks = hooks_builder.build_shared();

        let pending_done = tokio_util::task::TaskTracker::new();

        let finalizer = ResponseFinalizer::new(
            hooks.clone(),
            event_store.clone(),
            compactor.clone(),
            None,
            self.config.max_tokens,
        );

        Ok(AgentSession {
            runtime_ctx,
            event_store,
            session_store,
            config: self.config,
            system_prompt,
            hooks,
            compactor,
            pricing: None,
            finalizer,
            pending_done,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — pure functions, no builder state
// ---------------------------------------------------------------------------

/// Check if wiki is configured and minimally available.
///
/// Returns true if wiki config is enabled and the base path exists.
/// Does NOT initialize PageStore/PageIndex/WikiLog — that happens
/// during tool registration in `tools/builder.rs`.
fn is_wiki_available(config: &AgentConfig) -> bool {
    config.wiki.as_ref().map_or(false, |cfg| {
        cfg.enabled && std::path::Path::new(&cfg.base_path).exists()
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
