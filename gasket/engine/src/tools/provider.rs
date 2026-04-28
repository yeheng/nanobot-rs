//! Tool providers — decouple tool registration from `build_tool_registry`.
//!
//! Each subsystem (filesystem, wiki, system) implements `ToolProvider` and
//! registers its own tools. `build_tool_registry` only orchestrates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Config;
use crate::SubagentSpawner;
use crate::{MaintenanceStore, SessionStore};
use gasket_wiki::{PageIndex, PageStore};

use super::{
    registry::ToolRegistry, ClearSessionTool, CreatePlanTool, EditFileTool, EvolutionConfig,
    EvolutionTool, ExecTool, HistoryQueryTool, ListDirTool, ReadFileTool, SearchSopsTool,
    SpawnParallelTool, SpawnTool, ToolMetadata, WebFetchTool, WebSearchTool, WikiDecayTool,
    WikiDeleteTool, WikiReadTool, WikiRefreshTool, WikiSearchTool, WikiWriteTool, WriteFileTool,
};

/// Trait for subsystems that provide tools to the registry.
pub trait ToolProvider: Send + Sync {
    /// Register this provider's tools into the given registry.
    fn register_tools(&self, registry: &mut ToolRegistry);
}

/// Register a tool with full metadata — used by all ToolProvider impls.
macro_rules! reg {
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
        // Safe read-only tools
        reg!(
            registry,
            ReadFileTool::new(self.allowed_dir.clone()),
            "Read File",
            "filesystem",
            ["read", "file"],
            false,
            false
        );
        reg!(
            registry,
            ListDirTool::new(self.allowed_dir.clone()),
            "List Directory",
            "filesystem",
            ["read", "directory"],
            false,
            false
        );
        reg!(
            registry,
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
            registry,
            WebSearchTool::new(Some(self.web_config.clone())),
            "Web Search",
            "web",
            ["search", "web"],
            false,
            false
        );

        // Dangerous mutating tools
        reg!(
            registry,
            WriteFileTool::new(self.allowed_dir.clone()),
            "Write File",
            "filesystem",
            ["write", "file"],
            true,
            true
        );
        reg!(
            registry,
            EditFileTool::new(self.allowed_dir.clone()),
            "Edit File",
            "filesystem",
            ["edit", "file"],
            true,
            true
        );
        reg!(
            registry,
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
            registry,
            SpawnTool::new(),
            "Spawn Subagent",
            "system",
            ["spawn", "agent"],
            false,
            false
        );
        reg!(
            registry,
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
    page_store: Option<PageStore>,
    page_index: Option<Arc<PageIndex>>,
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    model: Option<String>,
    planning_prompt: Option<String>,
}

impl WikiToolProvider {
    pub fn new(
        page_store: Option<PageStore>,
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
        let Some(ref store) = self.page_store else {
            return;
        };

        if let Some(ref index) = self.page_index {
            reg!(
                registry,
                WikiSearchTool::new(store.clone(), index.clone()),
                "Wiki Search",
                "wiki",
                ["search", "wiki"],
                false,
                false
            );
            reg!(
                registry,
                WikiWriteTool::new(store.clone()),
                "Wiki Write",
                "wiki",
                ["write", "wiki"],
                false,
                true
            );
            reg!(
                registry,
                WikiRefreshTool::new(store.clone(), index.clone()),
                "Wiki Refresh",
                "wiki",
                ["refresh", "wiki"],
                false,
                false
            );
            reg!(
                registry,
                SearchSopsTool::new(index.clone()),
                "Search SOPs",
                "wiki",
                ["search", "sop", "wiki"],
                false,
                false
            );
        }

        reg!(
            registry,
            WikiReadTool::new(store.clone()),
            "Wiki Read",
            "wiki",
            ["read", "wiki"],
            false,
            false
        );
        reg!(
            registry,
            WikiDecayTool::new(store.clone()),
            "Wiki Decay",
            "wiki",
            ["decay", "wiki"],
            false,
            true
        );
        reg!(
            registry,
            WikiDeleteTool::new(store.clone()),
            "Wiki Delete",
            "wiki",
            ["delete", "wiki"],
            true,
            true
        );

        if let (Some(ref prov), Some(ref mdl)) = (&self.provider, &self.model) {
            reg!(
                registry,
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
    page_store: Option<PageStore>,
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
    model: Option<String>,
    evolution_prompt: Option<String>,
    event_store: Option<gasket_storage::EventStore>,
}

impl SystemToolProvider {
    pub fn new(
        session_store: Option<SessionStore>,
        maintenance_store: Option<MaintenanceStore>,
        page_store: Option<PageStore>,
        provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
        model: Option<String>,
        evolution_prompt: Option<String>,
        event_store: Option<gasket_storage::EventStore>,
    ) -> Self {
        Self {
            session_store,
            maintenance_store,
            page_store,
            provider,
            model,
            evolution_prompt,
            event_store,
        }
    }
}

impl ToolProvider for SystemToolProvider {
    fn register_tools(&self, registry: &mut ToolRegistry) {
        if let (Some(ref ss), Some(ref ms), Some(ref prov), Some(ref mdl), Some(ref es)) = (
            &self.session_store,
            &self.maintenance_store,
            &self.provider,
            &self.model,
            &self.event_store,
        ) {
            reg!(
                registry,
                EvolutionTool::new(EvolutionConfig {
                    session_store: ss.clone(),
                    maintenance_store: ms.clone(),
                    provider: prov.clone(),
                    model: mdl.clone(),
                    page_store: self.page_store.clone(),
                    event_store: es.clone(),
                    default_threshold: 20,
                    evolution_prompt: self.evolution_prompt.clone(),
                }),
                "Evolution",
                "system",
                ["maintenance", "learning"],
                false,
                true
            );
        }

        if let Some(ref db) = self.session_store {
            reg!(
                registry,
                HistoryQueryTool::new(db.pool().clone()),
                "Query History",
                "wiki",
                ["history", "search", "sqlite"],
                false,
                false
            );
            reg!(
                registry,
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
