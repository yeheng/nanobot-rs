//! Tool registry builder for constructing a fully-configured `ToolRegistry`.
//!
//! This module centralizes tool registration logic so that it lives in the
//! `tools` module rather than being duplicated or scattered across gateway
//! and agent construction sites.

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::memory::SqliteStore;
use crate::SubagentSpawner;

use super::{
    CreatePlanTool, EditFileTool, EvolutionTool, ExecTool, HistoryQueryTool, ListDirTool,
    MemorizeTool, MemorySearchTool, ReadFileTool, SearchSopsTool, SpawnParallelTool, SpawnTool,
    Tool, ToolMetadata, ToolRegistry, WebFetchTool, WebSearchTool, WikiDecayTool, WikiReadTool,
    WikiRefreshTool, WikiSearchTool, WikiWriteTool, WriteFileTool,
};

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
    #[allow(dead_code)]
    pub page_store: Option<Arc<crate::wiki::PageStore>>,
    /// Optional wiki PageIndex for semantic search.
    #[allow(dead_code)]
    pub page_index: Option<Arc<crate::wiki::PageIndex>>,
    /// Optional LLM provider for plan-generation tools.
    pub provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    /// Model identifier for plan-generation tools.
    pub model: Option<String>,
}

/// Register a tool with metadata — one line per tool.
///
/// Eliminates the 10-line `register_with_metadata(Box::new(...), ToolMetadata {...})` boilerplate.
macro_rules! register_tool {
    ($registry:expr, $tool:expr, $display:literal, $cat:literal, [$($tag:literal),*], $approval:literal, $mutating:literal) => {
        $registry.register_with_metadata(
            Box::new($tool),
            ToolMetadata {
                display_name: $display.to_string(),
                category: $cat.to_string(),
                tags: vec![$($tag.to_string()),*],
                requires_approval: $approval,
                is_mutating: $mutating,
            },
        );
    };
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

    // Suppress unused warnings when tool-spawn feature is disabled
    let _ = &subagent_spawner;

    let restrict = config.tools.restrict_to_workspace;
    let allowed_dir = if restrict {
        Some(workspace.to_path_buf())
    } else {
        None
    };

    let exec_workspace = resolve_exec_workspace(&config, &workspace);

    let mut tools = ToolRegistry::new();

    // ── Safe read-only tools (no approval required) ───────────
    register_tool!(
        tools,
        ReadFileTool::new(allowed_dir.clone()),
        "Read File",
        "filesystem",
        ["read", "file"],
        false,
        false
    );
    register_tool!(
        tools,
        ListDirTool::new(allowed_dir.clone()),
        "List Directory",
        "filesystem",
        ["read", "directory"],
        false,
        false
    );
    register_tool!(
        tools,
        WebFetchTool::with_config(Some(config.tools.web.clone())).unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to create WebFetchTool with proxy config: {}. Using default.",
                e
            );
            WebFetchTool::new()
        }),
        "Web Fetch",
        "web",
        ["http", "fetch"],
        false,
        false
    );
    register_tool!(
        tools,
        WebSearchTool::new(Some(config.tools.web.clone())),
        "Web Search",
        "web",
        ["search", "web"],
        false,
        false
    );

    // ── Dangerous mutating tools (require approval) ───────────
    register_tool!(
        tools,
        WriteFileTool::new(allowed_dir.clone()),
        "Write File",
        "filesystem",
        ["write", "file"],
        true,
        true
    );
    register_tool!(
        tools,
        EditFileTool::new(allowed_dir.clone()),
        "Edit File",
        "filesystem",
        ["edit", "file"],
        true,
        true
    );
    register_tool!(
        tools,
        ExecTool::from_config(exec_workspace, &config.tools.exec, restrict),
        "Execute Command",
        "system",
        ["shell", "exec"],
        true,
        true
    );

    // ── Spawn tools ───────────────────────────────────────────
    register_tool!(
        tools,
        SpawnTool::new(),
        "Spawn Subagent",
        "system",
        ["spawn", "agent"],
        false,
        false
    );
    register_tool!(
        tools,
        SpawnParallelTool::new(),
        "Spawn Parallel",
        "system",
        ["spawn", "parallel", "agent"],
        false,
        false
    );

    // ── Wiki-based memory tools (only if page_store is configured) ──
    if let Some(ref store) = page_store {
        register_tool!(
            tools,
            MemorizeTool::new(store.clone()),
            "Memorize",
            "memory",
            ["write", "memory"],
            false,
            true
        );

        register_tool!(
            tools,
            MemorySearchTool::new(store.clone(), page_index.clone()),
            "Memory Search",
            "memory",
            ["search", "memory"],
            false,
            false
        );

        // Unified wiki tools (require both page_store and page_index)
        if let Some(ref index) = page_index {
            register_tool!(
                tools,
                WikiSearchTool::new(store.clone(), index.clone()),
                "Wiki Search",
                "memory",
                ["search", "wiki"],
                false,
                false
            );
            register_tool!(
                tools,
                WikiWriteTool::new(store.clone(), index.clone()),
                "Wiki Write",
                "memory",
                ["write", "wiki"],
                false,
                true
            );
            register_tool!(
                tools,
                WikiRefreshTool::new(store.clone(), index.clone()),
                "Wiki Refresh",
                "memory",
                ["refresh", "wiki"],
                false,
                false
            );
            register_tool!(
                tools,
                SearchSopsTool::new(index.clone()),
                "Search SOPs",
                "memory",
                ["search", "sop", "wiki"],
                false,
                false
            );
        }

        // Wiki read tool (only needs page_store)
        register_tool!(
            tools,
            WikiReadTool::new(store.clone()),
            "Wiki Read",
            "memory",
            ["read", "wiki"],
            false,
            false
        );

        // Wiki decay tool (downgrades stale pages)
        register_tool!(
            tools,
            WikiDecayTool::new(store.clone()),
            "Wiki Decay",
            "memory",
            ["decay", "wiki"],
            false,
            true
        );

        // Plan generation tool (requires provider + model)
        if let (Some(ref prov), Some(ref mdl)) = (&provider, &model) {
            register_tool!(
                tools,
                CreatePlanTool::new(prov.clone(), mdl.clone(), store.clone()),
                "Create Plan",
                "system",
                ["plan", "markdown"],
                false,
                true
            );
        }
    }

    // Evolution maintenance tool — background learning from conversations
    if let (Some(ref db), Some(ref prov), Some(ref mdl), Some(ref ps)) =
        (&sqlite_store, &provider, &model, &page_store)
    {
        register_tool!(
            tools,
            EvolutionTool::new(db.clone(), prov.clone(), mdl.clone(), Some(ps.clone()), 20),
            "Evolution",
            "system",
            ["maintenance", "learning"],
            false,
            true
        );
    }

    // History query tool — direct SQL query over session_events
    if let Some(ref db) = sqlite_store {
        register_tool!(
            tools,
            HistoryQueryTool::new(db.pool().clone()),
            "Query History",
            "memory",
            ["history", "search", "sqlite"],
            false,
            false
        );
    }

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
