//! Tool registry builder for constructing a fully-configured `ToolRegistry`.
//!
//! This module centralizes tool registration logic so that it lives in the
//! `tools` module rather than being duplicated or scattered across gateway
//! and agent construction sites.

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::SqliteStore;
use crate::SubagentSpawner;

use super::{CoreToolProvider, SystemToolProvider, ToolProvider, WikiToolProvider};
use super::{Tool, ToolMetadata, ToolRegistry};

/// Resolve the exec workspace directory from config or default to `$HOME/.gasket`.
///
/// Creates the directory if it doesn't exist.
pub fn resolve_exec_workspace(config: &Config, fallback: &Path) -> std::path::PathBuf {
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
pub struct ToolRegistryConfig {
    /// Application configuration reference.
    pub config: Config,
    /// Workspace path.
    pub workspace: std::path::PathBuf,
    /// Optional subagent spawner for the spawn tools.
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Extra tools to register (e.g. gateway-specific `MessageTool`, `CronTool`).
    pub extra_tools: Vec<(Box<dyn Tool>, ToolMetadata)>,
    /// SQLite store for history search (optional).
    pub sqlite_store: Option<SqliteStore>,
    /// Optional wiki PageStore for unified knowledge management.
    pub page_store: Option<Arc<crate::wiki::PageStore>>,
    /// Optional wiki PageIndex for semantic search.
    pub page_index: Option<Arc<crate::wiki::PageIndex>>,
    /// Optional LLM provider for plan-generation tools.
    pub provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    /// Model identifier for plan-generation tools.
    pub model: Option<String>,
}

/// Build a [`ToolRegistry`] with common tools shared across all modes.
///
/// This function registers all common tools (filesystem, web, memory, etc.) and
/// accepts extra tools via the `extra_tools` field for mode-specific additions.
pub fn build_tool_registry(registry_config: ToolRegistryConfig) -> ToolRegistry {
    let ToolRegistryConfig {
        config,
        workspace,
        subagent_spawner,
        extra_tools,
        sqlite_store,
        page_store,
        page_index,
        provider,
        model,
    } = registry_config;

    let mut tools = ToolRegistry::new();

    // ── Core tools (filesystem, web, exec, spawn) ─────────────
    CoreToolProvider::new(&config, &workspace, subagent_spawner).register_tools(&mut tools);

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

    // ── System/maintenance tools (conditional on sqlite_store) ─
    let (session_store, maintenance_store) = sqlite_store
        .map(|s| (Some(s.session_store()), Some(s.maintenance_store())))
        .unwrap_or((None, None));
    SystemToolProvider::new(
        session_store,
        maintenance_store,
        page_store,
        provider,
        model,
        prompts.evolution.clone(),
    )
    .register_tools(&mut tools);

    // Extra tools (e.g. gateway-specific MessageTool, CronTool)
    for (tool, metadata) in extra_tools {
        tools.register_with_metadata(tool, metadata);
    }

    // Discover external plugins from ~/.gasket/scripts/
    if let Err(e) = crate::plugin::discover_plugins(&mut tools, None) {
        tracing::warn!("Failed to discover script tools: {}", e);
    }

    tools
}
