//! Common registry building utilities shared between gateway and agent commands.
//!
//! This module eliminates duplicate registration logic for tools, skills, and markdown loading.

use std::sync::Arc;

use gasket_engine::agent::AgentConfig;
use gasket_engine::agent::SubagentManager;
use gasket_engine::config::{Config, ModelRegistry};
use gasket_engine::memory::SqliteStore;
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::tools::WebFetchTool;
use gasket_engine::tools::WebSearchTool;
use gasket_engine::tools::{
    EditFileTool, ExecTool, ListDirTool, MemorizeTool, MemorySearchTool, ReadFileTool,
    ToolMetadata, ToolRegistry, WriteFileTool,
};
use gasket_engine::tools::{SpawnParallelTool, SpawnTool};

/// CLI-level implementation of ModelResolver using ProviderRegistry + ModelRegistry.
///
/// This resolves model_id strings (e.g., "minimax", "minimax/abab6.5-chat",
/// or named profiles like "smart-assistant") to actual provider + config pairs
/// for subagent model switching.
pub struct CliModelResolver {
    pub provider_registry: ProviderRegistry,
    pub model_registry: ModelRegistry,
}

impl gasket_engine::agent::ModelResolver for CliModelResolver {
    fn resolve_model(
        &self,
        model_id: &str,
    ) -> Option<(
        std::sync::Arc<dyn gasket_engine::providers::LlmProvider>,
        gasket_engine::agent::AgentConfig,
    )> {
        // 1. Try to resolve from named model profiles (e.g., "smart-assistant")
        if let Some((_id, profile)) = self
            .model_registry
            .get_profile_with_fallback(Some(model_id))
        {
            let provider_name = profile.provider.clone();
            let provider = self.provider_registry.get_or_create(&provider_name).ok()?;

            let config = gasket_engine::agent::AgentConfig {
                model: profile.model.clone(),
                temperature: profile.temperature.unwrap_or(1.0),
                max_tokens: profile.max_tokens.unwrap_or(65536),
                ..Default::default()
            };

            return Some((provider, config));
        }

        // 2. Try "provider/model" format (e.g., "minimax/abab6.5-chat")
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider_name = parts[0];
            let model_name = parts[1];

            if let Ok(provider) = self.provider_registry.get_or_create(provider_name) {
                let config = gasket_engine::agent::AgentConfig {
                    model: model_name.to_string(),
                    ..Default::default()
                };
                return Some((provider, config));
            }
        }

        // 3. Try as bare provider name (e.g., "minimax" → use provider's default model)
        if let Ok(provider) = self.provider_registry.get_or_create(model_id) {
            let config = gasket_engine::agent::AgentConfig {
                model: provider.default_model().to_string(),
                ..Default::default()
            };
            return Some((provider, config));
        }

        None
    }
}

/// Resolve the exec workspace directory from config or default to $HOME/.gasket.
///
/// Creates the directory if it doesn't exist.
pub fn resolve_exec_workspace(config: &Config, fallback: &std::path::Path) -> std::path::PathBuf {
    let workspace_path = if let Some(ref ws) = config.tools.exec.workspace {
        std::path::PathBuf::from(ws)
    } else {
        // Default: $HOME/.gasket
        dirs::home_dir()
            .map(|h| h.join(".gasket"))
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
        subagent_timeout_secs: defaults.subagent_timeout_secs,
        session_idle_timeout_secs: defaults.session_idle_timeout_secs,
        summarization_prompt: None,
    }
}

/// Configuration for building tool registry
pub struct ToolRegistryConfig {
    /// Configuration reference
    pub config: Config,
    /// Workspace path
    pub workspace: std::path::PathBuf,
    /// Optional subagent manager for spawn tool
    pub subagent_manager: Option<Arc<SubagentManager>>,
    /// Extra tools to register (e.g., gateway-specific MessageTool, CronTool)
    pub extra_tools: Vec<(Box<dyn gasket_engine::tools::Tool>, ToolMetadata)>,
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
        subagent_manager,
        extra_tools,
        sqlite_store,
        model_registry,
        provider_registry,
    } = registry_config;

    // Suppress unused warnings when tool-spawn feature is disabled
    let _ = (&subagent_manager, &model_registry, &provider_registry);

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
    {
        let spawn_tool = SpawnTool::new();
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
        let spawn_parallel_tool = SpawnParallelTool::new();
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
    }

    // Memory search tool — use SQLite MetadataStore when available
    let memory_tool = if let Some(ref db) = sqlite_store {
        MemorySearchTool::with_store(gasket_engine::memory::MetadataStore::new(db.pool().clone()))
    } else {
        MemorySearchTool::new()
    };

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

    // Memorize tool for writing structured long-term memories
    tools.register_with_metadata(
        Box::new(MemorizeTool::new()),
        ToolMetadata {
            display_name: "Memorize".to_string(),
            category: "memory".to_string(),
            tags: vec!["write".to_string(), "memory".to_string()],
            requires_approval: false,
            is_mutating: true,
        },
    );

    // Extra tools (e.g., gateway-specific MessageTool, CronTool)
    for (tool, metadata) in extra_tools {
        tools.register_with_metadata(tool, metadata);
    }

    tools
}
