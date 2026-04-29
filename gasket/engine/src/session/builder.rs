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

/// Bundle of embedding-specific dependencies for session construction.
///
/// Groups the three embedding parameters that are always passed together,
/// keeping function signatures under the clippy::too_many_arguments limit.
#[cfg(feature = "embedding")]
pub struct EmbeddingContext {
    pub searcher: Arc<gasket_embedding::RecallSearcher>,
    pub indexer: gasket_embedding::EmbeddingIndexer,
    pub event_store_tx: Option<tokio::sync::broadcast::Sender<gasket_types::SessionEvent>>,
}

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
    /// Optional semantic recall searcher + indexer (embedding feature).
    #[cfg(feature = "embedding")]
    embedding_recall: Option<(
        Arc<gasket_embedding::RecallSearcher>,
        gasket_embedding::EmbeddingIndexer,
    )>,
    /// Optional shared broadcast sender so the AgentSession's EventStore
    /// shares the same channel as the embedding infrastructure.
    #[cfg(feature = "embedding")]
    event_store_tx: Option<tokio::sync::broadcast::Sender<gasket_types::SessionEvent>>,
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
            #[cfg(feature = "embedding")]
            embedding_recall: None,
            #[cfg(feature = "embedding")]
            event_store_tx: None,
        }
    }

    /// Attach embedding recall infrastructure (searcher + indexer).
    /// Required for semantic history recall and the `history_search` tool.
    #[cfg(feature = "embedding")]
    pub fn with_embedding_recall(
        mut self,
        searcher: Arc<gasket_embedding::RecallSearcher>,
        indexer: gasket_embedding::EmbeddingIndexer,
    ) -> Self {
        self.embedding_recall = Some((searcher, indexer));
        self
    }

    /// Share the broadcast sender from an external EventStore so that
    /// the AgentSession's EventStore and the embedding indexer listen
    /// on the same channel.
    #[cfg(feature = "embedding")]
    pub fn with_event_store_tx(
        mut self,
        tx: tokio::sync::broadcast::Sender<gasket_types::SessionEvent>,
    ) -> Self {
        self.event_store_tx = Some(tx);
        self
    }

    /// Build the complete `AgentSession`.
    ///
    /// All services are constructed in dependency order as local variables —
    /// the compiler guarantees every value is initialized before use.
    pub async fn build(self) -> Result<AgentSession, AgentError> {
        // ── 1. Storage layer ─────────────────────────────────────────
        let pool = self.sqlite_store.pool();
        let session_store = SessionStore::new(pool.clone());
        #[cfg(feature = "embedding")]
        let event_store = if let Some(tx) = self.event_store_tx {
            EventStore::with_pool_and_sender(pool, tx)
        } else {
            EventStore::new(pool)
        };
        #[cfg(not(feature = "embedding"))]
        let event_store = EventStore::new(pool);

        // ── 2. Kernel runtime context ────────────────────────────────
        let kernel_config = self.config.to_kernel_config();
        let runtime_ctx = RuntimeContext {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: kernel_config,
            spawner: None,
            token_tracker: None,
            checkpoint_callback: None,
            session_key: None,
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
        let stop_words_path = self.config.stop_words_path.clone();

        #[cfg(feature = "embedding")]
        let (hooks, embedding_indexer) = if let Some((searcher, indexer)) = self.embedding_recall {
            let mut builder = crate::session::history::builder::build_default_hooks_builder(
                Some(event_store.clone()),
                stop_words_path.clone(),
            );
            let recall_config = gasket_embedding::RecallConfig::default();
            builder = builder.with_hook(Arc::new(crate::hooks::HistoryRecallHook::new(
                searcher,
                recall_config,
            )));
            (builder.build_shared(), Some(indexer))
        } else {
            let hooks_builder = crate::session::history::builder::build_default_hooks_builder(
                Some(event_store.clone()),
                stop_words_path.clone(),
            );
            (hooks_builder.build_shared(), None)
        };

        #[cfg(not(feature = "embedding"))]
        let hooks = {
            let hooks_builder = crate::session::history::builder::build_default_hooks_builder(
                Some(event_store.clone()),
                stop_words_path,
            );
            hooks_builder.build_shared()
        };

        // ── 9. ContextBuilder — encapsulates all pipeline dependencies ──
        let context_builder = crate::session::history::builder::ContextBuilder::new(
            event_store,
            session_store,
            system_prompt,
            None,
            hooks,
            history_config,
        );

        let pending_done = tokio_util::task::TaskTracker::new();

        let finalizer = ResponseFinalizer::new(
            context_builder.hooks().clone(),
            context_builder.event_store().clone(),
            compactor.clone(),
            None,
            self.config.max_tokens,
        );

        Ok(AgentSession {
            runtime_ctx,
            config: self.config,
            context_builder,
            compactor,
            pricing: None,
            finalizer,
            pending_done,
            #[cfg(feature = "embedding")]
            embedding_indexer,
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
    config
        .wiki
        .as_ref()
        .is_some_and(|cfg| cfg.enabled && std::path::Path::new(&cfg.base_path).exists())
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

/// Build an AgentSession with embedding recall support.
#[cfg(feature = "embedding")]
pub async fn build_session_with_embedding(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    sqlite_store: Arc<gasket_storage::SqliteStore>,
    embedding: EmbeddingContext,
) -> Result<AgentSession, AgentError> {
    let mut builder = SessionBuilder::new(provider, workspace, config, tools, sqlite_store)
        .with_embedding_recall(embedding.searcher, embedding.indexer);
    if let Some(tx) = embedding.event_store_tx {
        builder = builder.with_event_store_tx(tx);
    }
    builder.build().await
}
