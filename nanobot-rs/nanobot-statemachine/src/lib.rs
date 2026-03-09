//! nanobot-statemachine: A state machine for multi-agent collaboration.
//!
//! This crate provides a configurable state machine that enables
//! multi-agent collaboration patterns through the nanobot framework.
//!
//! # Architecture
//!
//! The state machine subsystem consists of:
//! - **StateMachineEngine**: The central event-driven engine
//! - **StateMachineConfig**: Data-driven configuration for states, transitions, and roles
//! - **StateMachineStore**: SQLite persistence for tasks and audit logs
//! - **StallDetector**: Monitors task heartbeats and detects stalled tasks
//! - **Tools**: `state_machine_task` and `report_progress` for agent interaction
//!
//! # Quick Start
//!
//! Add to your `config.yaml`:
//!
//! ```yaml
//! state_machine:
//!   enabled: true
//!   config_path: "~/.nanobot/state_machine.yaml"
//!   soul_templates_path: "~/.nanobot/souls"
//! ```
//!
//! Example state machine configuration:
//!
//! ```yaml
//! initial_state: triage
//! terminal_states: [done]
//! active_states: [triage, planning, executing, review]
//! sync_roles: [taizi, zhongshu, menxia]
//! transitions:
//!   - from: pending
//!     to: triage
//!   - from: triage
//!     to: planning
//!   - from: planning
//!     to: executing
//!   - from: executing
//!     to: review
//!   - from: review
//!     to: done
//! state_roles:
//!   triage: taizi
//!   planning: zhongshu
//!   executing: ministry
//!   review: menxia
//! ```
//!
//! Then call [`bootstrap`] during initialization:
//!
//! ```ignore
//! let handle = state_machine::bootstrap(
//!     &config.state_machine,
//!     memory_store.pool().clone(),
//!     subagent_manager,
//!     &mut tool_registry,
//! ).await?;
//! ```

pub mod config_loader;
pub mod engine;
pub mod events;
pub mod models;
pub mod stall_detector;
pub mod store;
pub mod tools;
pub mod types;

// Re-exports for convenience
pub use config_loader::{load_from_file, load_from_json, load_from_yaml, load_soul_templates};
pub use engine::StateMachineEngine;
pub use events::StateMachineEvent;
pub use models::{FlowLogEntry, ProgressEntry, StateMachineTask, TaskPriority};
pub use stall_detector::StallDetector;
pub use store::StateMachineStore;
pub use tools::{ReportProgressTool, StateMachineTaskTool};
pub use types::{AgentRoleConfig, GateConfig, State, StateMachineConfig, Transition};

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::info;

use nanobot_core::agent::subagent::SubagentManager;
use nanobot_core::tools::ToolRegistry;

/// State machine subsystem handle.
///
/// Returned by [`bootstrap`] when the state machine is successfully initialized.
pub struct StateMachineHandle {
    /// Sender for state machine events (cloneable).
    pub event_tx: mpsc::Sender<StateMachineEvent>,
    /// The resolved state machine configuration.
    pub config: Arc<StateMachineConfig>,
    /// The store for state machine entities.
    pub store: StateMachineStore,
}

/// Bootstrap configuration for the state machine subsystem.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateMachineBootstrapConfig {
    /// Master switch — the state machine subsystem is completely dormant when false.
    #[serde(default)]
    pub enabled: bool,

    /// Path to the configuration file (YAML or JSON).
    #[serde(default)]
    pub config_path: Option<String>,

    /// Path to soul templates directory.
    #[serde(default)]
    pub soul_templates_path: Option<String>,

    /// Use the built-in 三省六部 preset as default.
    #[serde(default = "default_true", alias = "useDefaultTemplate")]
    pub use_default_template: bool,
}

fn default_true() -> bool {
    true
}

impl Default for StateMachineBootstrapConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            config_path: None,
            soul_templates_path: None,
            use_default_template: true,
        }
    }
}

/// Initialize the state machine subsystem.
///
/// Returns `None` if state machine is disabled in config.
///
/// # Arguments
///
/// * `bootstrap_config` - State machine bootstrap configuration
/// * `pool` - SQLite pool shared with the main store
/// * `subagent_manager` - Manager for spawning sub-agents
/// * `tool_registry` - Tool registry to register state machine tools
///
/// # Example
///
/// ```ignore
/// let handle = state_machine::bootstrap(
///     &config.state_machine.unwrap_or_default(),
///     memory_store.pool().clone(),
///     subagent_manager,
///     &mut tool_registry,
/// ).await?;
/// ```
pub async fn bootstrap(
    bootstrap_config: &StateMachineBootstrapConfig,
    pool: sqlx::SqlitePool,
    subagent_manager: Arc<SubagentManager>,
    tool_registry: &mut ToolRegistry,
) -> anyhow::Result<Option<StateMachineHandle>> {
    if !bootstrap_config.enabled {
        info!("State machine subsystem disabled");
        return Ok(None);
    }

    info!("Initializing state machine subsystem...");

    // 1. Load or create configuration
    let config = if let Some(path) = &bootstrap_config.config_path {
        let path = std::path::Path::new(path);
        let loaded = load_from_file(path)?;
        loaded.validate().map_err(|errors| {
            anyhow::anyhow!("Validation failed:\n  - {}", errors.join("\n  - "))
        })?;
        loaded
    } else if bootstrap_config.use_default_template {
        StateMachineConfig::default()
    } else {
        anyhow::bail!(
            "State machine is enabled but no configuration is provided \
             (set config_path or use_default_template=true)"
        );
    };

    info!(
        "Loaded state machine config (initial_state={}, terminal_states={:?})",
        config.initial_state, config.terminal_states
    );

    // 2. Initialize the store
    let store = StateMachineStore::new(pool);
    store.init_tables().await?;

    // 3. Load soul templates
    let soul_templates = bootstrap_config
        .soul_templates_path
        .as_ref()
        .map(|p| load_soul_templates(std::path::Path::new(p)))
        .unwrap_or_default();

    if soul_templates.is_empty() {
        info!("No soul templates loaded (using default prompts)");
    } else {
        info!("Loaded {} soul template(s)", soul_templates.len());
    }

    // 4. Create event channel
    let (event_tx, event_rx) = mpsc::channel(256);

    // 5. Spawn the engine
    let engine = StateMachineEngine::new(
        store.clone(),
        subagent_manager,
        config.clone(),
        event_tx.clone(),
        event_rx,
        soul_templates,
    );
    tokio::spawn(engine.run());

    // 6. Spawn the stall detector
    let detector = StallDetector::new(
        store.clone(),
        event_tx.clone(),
        config.stall_timeout_secs,
        config.active_states.clone(),
    );
    tokio::spawn(detector.run());

    // 7. Register tools
    let config_arc = Arc::new(config.clone());
    tool_registry.register(Box::new(StateMachineTaskTool::new(
        store.clone(),
        event_tx.clone(),
        config_arc.clone(),
    )) as Box<dyn nanobot_core::tools::Tool>);
    tool_registry.register(
        Box::new(ReportProgressTool::new(store.clone(), event_tx.clone()))
            as Box<dyn nanobot_core::tools::Tool>,
    );

    info!(
        "State machine subsystem initialized (initial_state={}, terminal_states={:?})",
        config.initial_state, config.terminal_states
    );

    Ok(Some(StateMachineHandle {
        event_tx,
        config: config_arc,
        store,
    }))
}
