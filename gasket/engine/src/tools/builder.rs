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
    CreatePlanTool, EditFileTool, ExecTool, HistoryQueryTool, ListDirTool, MemorizeTool,
    MemorySearchTool, ReadFileTool, SearchSopsTool, SpawnParallelTool, SpawnTool, Tool,
    ToolMetadata, ToolRegistry, WebFetchTool, WebSearchTool, WikiDecayTool, WikiReadTool,
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

    // Safe read-only tools (no approval required)
    tools.register_with_metadata(
        Box::new(ReadFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Read File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["read".to_string(), "file".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(ListDirTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "List Directory".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["read".to_string(), "directory".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(
            WebFetchTool::with_config(Some(config.tools.web.clone())).unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to create WebFetchTool with proxy config: {}. Using default.",
                    e
                );
                WebFetchTool::new()
            }),
        ),
        ToolMetadata {
            display_name: "Web Fetch".to_string(),
            category: "web".to_string(),
            tags: vec!["http".to_string(), "fetch".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(WebSearchTool::new(Some(config.tools.web.clone()))),
        ToolMetadata {
            display_name: "Web Search".to_string(),
            category: "web".to_string(),
            tags: vec!["search".to_string(), "web".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // Dangerous mutating tools (require approval)
    tools.register_with_metadata(
        Box::new(WriteFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Write File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["write".to_string(), "file".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );
    tools.register_with_metadata(
        Box::new(EditFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Edit File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["edit".to_string(), "file".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );
    tools.register_with_metadata(
        Box::new(ExecTool::from_config(
            exec_workspace,
            &config.tools.exec,
            restrict,
        )),
        ToolMetadata {
            display_name: "Execute Command".to_string(),
            category: "system".to_string(),
            tags: vec!["shell".to_string(), "exec".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );

    // Spawn tools
    tools.register_with_metadata(
        Box::new(SpawnTool::new()),
        ToolMetadata {
            display_name: "Spawn Subagent".to_string(),
            category: "system".to_string(),
            tags: vec!["spawn".to_string(), "agent".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(SpawnParallelTool::new()),
        ToolMetadata {
            display_name: "Spawn Parallel".to_string(),
            category: "system".to_string(),
            tags: vec![
                "spawn".to_string(),
                "parallel".to_string(),
                "agent".to_string(),
            ],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // Wiki-based memory tools (only if page_store is configured)
    if let Some(ref store) = page_store {
        // Memorize tool
        tools.register_with_metadata(
            Box::new(MemorizeTool::new(store.clone())),
            ToolMetadata {
                display_name: "Memorize".to_string(),
                category: "memory".to_string(),
                tags: vec!["write".to_string(), "memory".to_string()],
                requires_approval: false,
                is_mutating: true,
            },
        );

        // Memory search tool
        let search_tool = MemorySearchTool::new(store.clone(), page_index.clone());
        tools.register_with_metadata(
            Box::new(search_tool),
            ToolMetadata {
                display_name: "Memory Search".to_string(),
                category: "memory".to_string(),
                tags: vec!["search".to_string(), "memory".to_string()],
                requires_approval: false,
                is_mutating: false,
            },
        );

        // Unified wiki tools (require both page_store and page_index)
        if let Some(ref index) = page_index {
            tools.register_with_metadata(
                Box::new(WikiSearchTool::new(store.clone(), index.clone())),
                ToolMetadata {
                    display_name: "Wiki Search".to_string(),
                    category: "memory".to_string(),
                    tags: vec!["search".to_string(), "wiki".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            );

            tools.register_with_metadata(
                Box::new(WikiWriteTool::new(store.clone(), index.clone())),
                ToolMetadata {
                    display_name: "Wiki Write".to_string(),
                    category: "memory".to_string(),
                    tags: vec!["write".to_string(), "wiki".to_string()],
                    requires_approval: false,
                    is_mutating: true,
                },
            );

            tools.register_with_metadata(
                Box::new(WikiRefreshTool::new(store.clone(), index.clone())),
                ToolMetadata {
                    display_name: "Wiki Refresh".to_string(),
                    category: "memory".to_string(),
                    tags: vec!["refresh".to_string(), "wiki".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            );

            // SOP search tool (filters wiki search to SOP pages only)
            tools.register_with_metadata(
                Box::new(SearchSopsTool::new(index.clone())),
                ToolMetadata {
                    display_name: "Search SOPs".to_string(),
                    category: "memory".to_string(),
                    tags: vec!["search".to_string(), "sop".to_string(), "wiki".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            );
        }

        // Wiki read tool (only needs page_store)
        tools.register_with_metadata(
            Box::new(WikiReadTool::new(store.clone())),
            ToolMetadata {
                display_name: "Wiki Read".to_string(),
                category: "memory".to_string(),
                tags: vec!["read".to_string(), "wiki".to_string()],
                requires_approval: false,
                is_mutating: false,
            },
        );

        // Wiki decay tool (downgrades stale pages)
        tools.register_with_metadata(
            Box::new(WikiDecayTool::new(store.clone())),
            ToolMetadata {
                display_name: "Wiki Decay".to_string(),
                category: "memory".to_string(),
                tags: vec!["decay".to_string(), "wiki".to_string()],
                requires_approval: false,
                is_mutating: true,
            },
        );

        // Plan generation tool (requires provider + model)
        if let (Some(ref prov), Some(ref mdl)) = (&provider, &model) {
            tools.register_with_metadata(
                Box::new(CreatePlanTool::new(
                    prov.clone(),
                    mdl.clone(),
                    store.clone(),
                )),
                ToolMetadata {
                    display_name: "Create Plan".to_string(),
                    category: "system".to_string(),
                    tags: vec!["plan".to_string(), "markdown".to_string()],
                    requires_approval: false,
                    is_mutating: true,
                },
            );
        }
    }

    // History query tool — direct SQL query over session_events
    // History query tool — direct SQL query over session_events
    if let Some(ref db) = sqlite_store {
        tools.register_with_metadata(
            Box::new(HistoryQueryTool::new(db.pool().clone())),
            ToolMetadata {
                display_name: "Query History".to_string(),
                category: "memory".to_string(),
                tags: vec![
                    "history".to_string(),
                    "search".to_string(),
                    "sqlite".to_string(),
                ],
                requires_approval: false,
                is_mutating: false,
            },
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
