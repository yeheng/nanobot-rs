//! Tool registry builder for constructing a fully-configured `ToolRegistry`.
//!
//! This module centralizes tool registration logic so that it lives in the
//! `tools` module rather than being duplicated or scattered across gateway
//! and agent construction sites.

use std::path::Path;
use std::sync::Arc;

use crate::SubagentSpawner;

use super::{CoreToolProvider, SystemToolProvider, ToolProvider, WikiToolProvider};
use super::{Tool, ToolMetadata, ToolRegistry};

/// Resolve the exec workspace directory from config or default to `$HOME/.gasket`.
///
/// Creates the directory if it doesn't exist.
pub fn resolve_exec_workspace(
    config: &crate::config::Config,
    fallback: &Path,
) -> std::path::PathBuf {
    let workspace_path = if let Some(ref ws) = config.tools.exec.workspace {
        std::path::PathBuf::from(ws)
    } else {
        dirs::home_dir()
            .map(|h| h.join(".gasket"))
            .unwrap_or_else(|| fallback.to_path_buf())
    };

    if !workspace_path.exists() {
        if let Err(e) = std::fs::create_dir_all(&workspace_path) {
            tracing::warn!(
                "Failed to create exec workspace {:?}: {}. Falling back to {:?}",
                workspace_path,
                e,
                fallback
            );
            return fallback.to_path_buf();
        }
        tracing::info!("Created exec workspace: {:?}", workspace_path);
    }

    workspace_path
}

/// Configuration for building a [`ToolRegistry`].
///
/// Only dynamic / mode-specific dependencies are kept as fields.
/// Infra singletons (config, database) are fetched from globals inside
/// [`build_tool_registry`].
pub struct ToolRegistryConfig {
    /// Optional subagent spawner for the spawn tools.
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Extra tools to register (e.g. gateway-specific `MessageTool`, `CronTool`).
    pub extra_tools: Vec<(Box<dyn Tool>, ToolMetadata)>,
    /// Optional wiki PageStore for unified knowledge management.
    pub page_store: Option<gasket_wiki::PageStore>,
    /// Optional wiki PageIndex for semantic search.
    pub page_index: Option<Arc<gasket_wiki::PageIndex>>,
    /// Optional LLM provider for plan-generation tools.
    pub provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    /// Model identifier for plan-generation tools.
    pub model: Option<String>,
    /// Optional semantic history search (embedding feature).
    #[cfg(feature = "embedding")]
    pub history_search: Option<HistorySearchParams>,
}

/// Parameters needed to construct the `history_search` tool.
#[cfg(feature = "embedding")]
pub struct HistorySearchParams {
    pub searcher: std::sync::Arc<gasket_embedding::RecallSearcher>,
    pub config: gasket_embedding::RecallConfig,
}

/// Build a [`ToolRegistry`] with common tools shared across all modes.
///
/// This function registers all common tools (filesystem, web, memory, etc.) and
/// accepts extra tools via the `extra_tools` field for mode-specific additions.
/// Infra singletons (config, database) are read from globals — they must be
/// initialized by the caller before invoking this function.
pub fn build_tool_registry(registry_config: ToolRegistryConfig) -> ToolRegistry {
    let ToolRegistryConfig {
        subagent_spawner,
        extra_tools,
        page_store,
        page_index,
        provider,
        model,
        #[cfg(feature = "embedding")]
        history_search,
    } = registry_config;

    let config = crate::config::get_config();
    let workspace = resolve_exec_workspace(config, std::path::Path::new("."));
    let sqlite_store = gasket_storage::get_db();

    let mut tools = ToolRegistry::new();

    // ── Core tools (filesystem, web, exec, spawn) ─────────────
    CoreToolProvider::new(config, &workspace, subagent_spawner).register_tools(&mut tools);

    // ── Wiki + memory tools (conditional on page_store) ───────
    let prompts = &config.agents.defaults.prompts;
    WikiToolProvider::new(
        page_store.clone(),
        page_index.clone(),
        provider.clone(),
        model.clone(),
        prompts.planning.clone(),
    )
    .register_tools(&mut tools);

    // ── System/maintenance tools ─
    let session_store = Some(sqlite_store.session_store());
    let maintenance_store = Some(sqlite_store.maintenance_store());
    let event_store = Some(gasket_storage::EventStore::new(sqlite_store.pool()));
    SystemToolProvider::new(
        session_store,
        maintenance_store,
        page_store,
        provider.clone(),
        model,
        prompts.evolution.clone(),
        event_store,
    )
    .register_tools(&mut tools);

    // Extra tools (e.g. gateway-specific MessageTool, CronTool)
    for (tool, metadata) in extra_tools {
        tools.register_with_metadata(tool, metadata);
    }

    // ── Embedding-based history search (conditional) ───────────
    #[cfg(feature = "embedding")]
    {
        use super::HistorySearchTool;
        if let Some(params) = history_search {
            tools.register(Box::new(HistorySearchTool::new(
                params.searcher,
                params.config,
            )));
        }
    }

    // Discover external plugins — engine resources are injected at construction time.
    let engine_resources = provider.map(|p| {
        let tools_arc = Arc::new(tools.clone());
        crate::plugin::EngineResources {
            tool_registry: tools_arc,
            provider: p,
        }
    });
    if let Err(e) = crate::plugin::discover_plugins(&mut tools, engine_resources) {
        tracing::warn!("Failed to discover script tools: {}", e);
    }

    tools
}
