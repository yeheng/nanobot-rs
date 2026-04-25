//! Tool providers — decouple tool registration from `build_tool_registry`.
//!
//! Each subsystem (filesystem, wiki, system) implements `ToolProvider` and
//! registers its own tools. `build_tool_registry` only orchestrates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Config;
use crate::wiki::{PageIndex, PageStore};
use crate::SubagentSpawner;
use crate::{MaintenanceStore, SessionStore};

use super::{
    registry::ToolRegistry, ClearSessionTool, CreatePlanTool, EditFileTool, EvolutionTool,
    ExecTool, HistoryQueryTool, ListDirTool, ReadFileTool, SearchSopsTool, SpawnParallelTool,
    SpawnTool, ToolMetadata, WebFetchTool, WebSearchTool, WikiDecayTool, WikiReadTool,
    WikiRefreshTool, WikiSearchTool, WikiWriteTool, WriteFileTool,
};

/// Trait for subsystems that provide tools to the registry.
pub trait ToolProvider: Send + Sync {
    /// Register this provider's tools into the given registry.
    fn register_tools(&self, registry: &mut ToolRegistry);
}

// ---------------------------------------------------------------------------
// CoreToolProvider — filesystem, web, exec, spawn
// ---------------------------------------------------------------------------

/// Provides core tools that are always available.
pub struct CoreToolProvider {
    restrict: bool,
    allowed_dir: Option<PathBuf>,
    exec_workspace: PathBuf,
    web_config: crate::config::WebToolsConfig,
    exec_config: crate::config::ExecToolConfig,
    _subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
}

impl CoreToolProvider {
    pub fn new(
        config: &Config,
        workspace: &Path,
        subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    ) -> Self {
        let restrict = config.tools.restrict_to_workspace;
        let allowed_dir = if restrict {
            Some(workspace.to_path_buf())
        } else {
            None
        };
        let exec_workspace = super::builder::resolve_exec_workspace(config, workspace);
        Self {
            restrict,
            allowed_dir,
            exec_workspace,
            web_config: config.tools.web.clone(),
            exec_config: config.tools.exec.clone(),
            _subagent_spawner: subagent_spawner,
        }
    }
}

impl ToolProvider for CoreToolProvider {
    fn register_tools(&self, registry: &mut ToolRegistry) {
        macro_rules! reg {
            ($tool:expr, $display:literal, $cat:literal, [$($tag:literal),*], $approval:literal, $mutating:literal) => {
                registry.register_with_metadata(
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

        // Safe read-only tools
        reg!(
            ReadFileTool::new(self.allowed_dir.clone()),
            "Read File",
            "filesystem",
            ["read", "file"],
            false,
            false
        );
        reg!(
            ListDirTool::new(self.allowed_dir.clone()),
            "List Directory",
            "filesystem",
            ["read", "directory"],
            false,
            false
        );
        reg!(
            WebFetchTool::with_config(Some(self.web_config.clone())).unwrap_or_else(|e| {
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
        reg!(
            WebSearchTool::new(Some(self.web_config.clone())),
            "Web Search",
            "web",
            ["search", "web"],
            false,
            false
        );

        // Dangerous mutating tools
        reg!(
            WriteFileTool::new(self.allowed_dir.clone()),
            "Write File",
            "filesystem",
            ["write", "file"],
            true,
            true
        );
        reg!(
            EditFileTool::new(self.allowed_dir.clone()),
            "Edit File",
            "filesystem",
            ["edit", "file"],
            true,
            true
        );
        reg!(
            ExecTool::from_config(
                self.exec_workspace.clone(),
                &self.exec_config,
                self.restrict
            ),
            "Execute Command",
            "system",
            ["shell", "exec"],
            true,
            true
        );

        // Spawn tools
        reg!(
            SpawnTool::new(),
            "Spawn Subagent",
            "system",
            ["spawn", "agent"],
            false,
            false
        );
        reg!(
            SpawnParallelTool::new(),
            "Spawn Parallel",
            "system",
            ["spawn", "parallel", "agent"],
            false,
            false
        );
    }
}

// ---------------------------------------------------------------------------
// WikiToolProvider — wiki + memory tools
// ---------------------------------------------------------------------------

/// Provides wiki and memory tools (conditional on page_store).
pub struct WikiToolProvider {
    page_store: Option<Arc<PageStore>>,
    page_index: Option<Arc<PageIndex>>,
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    model: Option<String>,
    planning_prompt: Option<String>,
}

impl WikiToolProvider {
    pub fn new(
        page_store: Option<Arc<PageStore>>,
        page_index: Option<Arc<PageIndex>>,
        provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
        model: Option<String>,
        planning_prompt: Option<String>,
    ) -> Self {
        Self {
            page_store,
            page_index,
            provider,
            model,
            planning_prompt,
        }
    }
}

impl ToolProvider for WikiToolProvider {
    fn register_tools(&self, registry: &mut ToolRegistry) {
        macro_rules! reg {
            ($tool:expr, $display:literal, $cat:literal, [$($tag:literal),*], $approval:literal, $mutating:literal) => {
                registry.register_with_metadata(
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

        let Some(ref store) = self.page_store else {
            return;
        };

        if let Some(ref index) = self.page_index {
            reg!(
                WikiSearchTool::new(store.clone(), index.clone()),
                "Wiki Search",
                "memory",
                ["search", "wiki"],
                false,
                false
            );
            reg!(
                WikiWriteTool::new(store.clone()),
                "Wiki Write",
                "memory",
                ["write", "wiki"],
                false,
                true
            );
            reg!(
                WikiRefreshTool::new(store.clone(), index.clone()),
                "Wiki Refresh",
                "memory",
                ["refresh", "wiki"],
                false,
                false
            );
            reg!(
                SearchSopsTool::new(index.clone()),
                "Search SOPs",
                "memory",
                ["search", "sop", "wiki"],
                false,
                false
            );
        }

        reg!(
            WikiReadTool::new(store.clone()),
            "Wiki Read",
            "memory",
            ["read", "wiki"],
            false,
            false
        );
        reg!(
            WikiDecayTool::new(store.clone()),
            "Wiki Decay",
            "memory",
            ["decay", "wiki"],
            false,
            true
        );

        if let (Some(ref prov), Some(ref mdl)) = (&self.provider, &self.model) {
            reg!(
                CreatePlanTool::new(
                    prov.clone(),
                    mdl.clone(),
                    store.clone(),
                    self.planning_prompt.clone(),
                ),
                "Create Plan",
                "system",
                ["plan", "markdown"],
                false,
                true
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SystemToolProvider — evolution, history query, maintenance
// ---------------------------------------------------------------------------

/// Provides system/maintenance tools (conditional on store and provider).
pub struct SystemToolProvider {
    session_store: Option<SessionStore>,
    maintenance_store: Option<MaintenanceStore>,
    page_store: Option<Arc<PageStore>>,
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    model: Option<String>,
    evolution_prompt: Option<String>,
}

impl SystemToolProvider {
    pub fn new(
        session_store: Option<SessionStore>,
        maintenance_store: Option<MaintenanceStore>,
        page_store: Option<Arc<PageStore>>,
        provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
        model: Option<String>,
        evolution_prompt: Option<String>,
    ) -> Self {
        Self {
            session_store,
            maintenance_store,
            page_store,
            provider,
            model,
            evolution_prompt,
        }
    }
}

impl ToolProvider for SystemToolProvider {
    fn register_tools(&self, registry: &mut ToolRegistry) {
        macro_rules! reg {
            ($tool:expr, $display:literal, $cat:literal, [$($tag:literal),*], $approval:literal, $mutating:literal) => {
                registry.register_with_metadata(
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

        if let (Some(ref ss), Some(ref ms), Some(ref prov), Some(ref mdl)) = (
            &self.session_store,
            &self.maintenance_store,
            &self.provider,
            &self.model,
        ) {
            reg!(
                EvolutionTool::new(
                    ss.clone(),
                    ms.clone(),
                    prov.clone(),
                    mdl.clone(),
                    self.page_store.clone(),
                    20,
                    self.evolution_prompt.clone(),
                ),
                "Evolution",
                "system",
                ["maintenance", "learning"],
                false,
                true
            );
        }

        if let Some(ref db) = self.session_store {
            reg!(
                HistoryQueryTool::new(db.pool().clone()),
                "Query History",
                "memory",
                ["history", "search", "sqlite"],
                false,
                false
            );
            reg!(
                ClearSessionTool::new(db.clone()),
                "Clear Session History",
                "system",
                ["session", "cleanup", "history"],
                true,
                true
            );
        }
    }
}
