//! Common registry building utilities shared between gateway and agent commands.
//!
//! This module eliminates duplicate registration logic for tools, skills, and markdown loading.

use std::sync::Arc;

use nanobot_core::agent::subagent::SubagentManager;
use nanobot_core::agent::AgentConfig;
use nanobot_core::config::{Config, ModelRegistry};
use nanobot_core::memory::SqliteStore;
use nanobot_core::providers::ProviderRegistry;
use nanobot_core::tools::{
    EditFileTool, ExecTool, HistorySearchTool, ListDirTool, MemorySearchTool, ReadFileTool,
    SpawnParallelTool, SpawnTool, ToolMetadata, ToolRegistry, WebFetchTool, WebSearchTool,
    WriteFileTool,
};

/// Resolve the exec workspace directory from config or default to $HOME/.nanobot.
///
/// Creates the directory if it doesn't exist.
pub fn resolve_exec_workspace(config: &Config, fallback: &std::path::Path) -> std::path::PathBuf {
    let workspace_path = if let Some(ref ws) = config.tools.exec.workspace {
        std::path::PathBuf::from(ws)
    } else {
        // Default: $HOME/.nanobot
        dirs::home_dir()
            .map(|h| h.join(".nanobot"))
            .unwrap_or_else(|| fallback.to_path_buf())
    };

    // Ensure the directory exists
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

/// Build AgentConfig from the config file, applying defaults for zero-valued fields.
pub fn build_agent_config(config: &Config) -> AgentConfig {
    let defaults = AgentConfig::default();
    AgentConfig {
        model: String::new(), // caller overrides with resolved model
        max_iterations: match config.agents.defaults.max_iterations {
            0 => defaults.max_iterations,
            v => v,
        },
        temperature: config.agents.defaults.temperature,
        max_tokens: match config.agents.defaults.max_tokens {
            0 => defaults.max_tokens,
            v => v,
        },
        memory_window: match config.agents.defaults.memory_window {
            0 => defaults.memory_window,
            v => v,
        },
        max_tool_result_chars: defaults.max_tool_result_chars,
        thinking_enabled: config.agents.defaults.thinking_enabled,
        streaming: config.agents.defaults.streaming,
    }
}

/// Configuration for building tool registry
pub struct ToolRegistryConfig {
    /// Configuration reference
    pub config: Config,
    /// Workspace path
    pub workspace: std::path::PathBuf,
    /// MCP tools loaded from external servers
    pub mcp_tools: Vec<Box<dyn nanobot_core::tools::Tool>>,
    /// Optional subagent manager for spawn tool
    pub subagent_manager: Option<Arc<SubagentManager>>,
    /// Extra tools to register (e.g., gateway-specific MessageTool, CronTool)
    pub extra_tools: Vec<(Box<dyn nanobot_core::tools::Tool>, ToolMetadata)>,
    /// SQLite store for history search (optional)
    pub sqlite_store: Option<SqliteStore>,
    /// Model registry for switch_model tool (optional)
    pub model_registry: Option<Arc<ModelRegistry>>,
    /// Provider registry for switch_model tool (optional)
    pub provider_registry: Option<Arc<ProviderRegistry>>,
}

/// Build tool registry with common tools shared between CLI and gateway modes.
///
/// This function registers all common tools (filesystem, web, memory, etc.) and
/// accepts extra tools via `extra_tools` parameter for mode-specific tools.
///
/// # Arguments
/// * `registry_config` - Configuration for building the registry
///
/// # Returns
/// A configured `ToolRegistry` ready for use
pub fn build_tool_registry(registry_config: ToolRegistryConfig) -> ToolRegistry {
    let ToolRegistryConfig {
        config,
        workspace,
        mcp_tools,
        subagent_manager,
        extra_tools,
        sqlite_store,
        model_registry,
        provider_registry,
    } = registry_config;

    let restrict = config.tools.restrict_to_workspace;
    let allowed_dir = if restrict {
        Some(workspace.to_path_buf())
    } else {
        None
    };

    // Resolve exec workspace directory
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

    // Spawn tool
    let spawn_tool = match (&subagent_manager, &model_registry, &provider_registry) {
        (Some(mgr), Some(model_reg), Some(provider_reg)) => SpawnTool::with_registries(
            Arc::clone(mgr),
            Arc::clone(model_reg),
            Arc::clone(provider_reg),
        ),
        (Some(mgr), _, _) => SpawnTool::with_manager(Arc::clone(mgr)),
        _ => SpawnTool::new(),
    };
    tools.register_with_metadata(
        Box::new(spawn_tool),
        ToolMetadata {
            display_name: "Spawn Subagent".to_string(),
            category: "system".to_string(),
            tags: vec!["spawn".to_string(), "agent".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // Spawn parallel tool
    let spawn_parallel_tool = match (&subagent_manager, &model_registry, &provider_registry) {
        (Some(mgr), Some(model_reg), Some(provider_reg)) => SpawnParallelTool::with_registries(
            Arc::clone(mgr),
            Arc::clone(model_reg),
            Arc::clone(provider_reg),
        ),
        (Some(mgr), _, _) => SpawnParallelTool::with_manager(Arc::clone(mgr)),
        _ => SpawnParallelTool::new(),
    };
    tools.register_with_metadata(
        Box::new(spawn_parallel_tool),
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

    // MCP tools (metadata assigned by MCP manager)
    for mcp_tool in mcp_tools {
        tools.register(mcp_tool);
    }

    // Memory search tool using filesystem-based search
    // For advanced Tantivy-based full-text search, use the standalone tantivy-mcp server
    let memory_tool = MemorySearchTool::new();

    tools.register_with_metadata(
        Box::new(memory_tool),
        ToolMetadata {
            display_name: "Memory Search".to_string(),
            category: "memory".to_string(),
            tags: vec!["search".to_string(), "memory".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // History search tool using SQLite database
    if let Some(db) = sqlite_store {
        let history_tool = HistorySearchTool::new(db, config.tools.history_search_limit);
        tools.register_with_metadata(
            Box::new(history_tool),
            ToolMetadata {
                display_name: "History Search".to_string(),
                category: "search".to_string(),
                tags: vec![
                    "search".to_string(),
                    "history".to_string(),
                    "sqlite".to_string(),
                ],
                requires_approval: false,
                is_mutating: false,
            },
        );
    }

    // Extra tools (e.g., gateway-specific MessageTool, CronTool)
    for (tool, metadata) in extra_tools {
        tools.register_with_metadata(tool, metadata);
    }

    tools
}
