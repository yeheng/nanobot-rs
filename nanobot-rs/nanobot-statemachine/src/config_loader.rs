//! Configuration loader for the state machine.
//!
//! Supports loading from YAML or JSON files with optional
//! fallback to built-in defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

// Re-export types for convenience
pub use crate::types::{AgentRoleConfig, GateConfig, State, StateMachineConfig, Transition};

/// Load state machine configuration from a YAML file.
pub fn load_from_yaml(path: &Path) -> anyhow::Result<StateMachineConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: StateMachineConfig = serde_yaml::from_str(&content)?;
    Ok(config)
}

/// Load state machine configuration from a JSON file.
pub fn load_from_json(path: &Path) -> anyhow::Result<StateMachineConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: StateMachineConfig = serde_json::from_str(&content)?;
    Ok(config)
}

/// Load state machine configuration from a file (auto-detect format).
///
/// Detects format based on file extension:
/// - `.yaml` or `.yml` → YAML
/// - `.json` → JSON
pub fn load_from_file(path: &Path) -> anyhow::Result<StateMachineConfig> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("yaml") | Some("yml") => load_from_yaml(path),
        Some("json") => load_from_json(path),
        Some(other) => Err(anyhow::anyhow!(
            "Unsupported configuration file extension: '.{}'. Use .yaml, .yml, or .json",
            other
        )),
        None => Err(anyhow::anyhow!(
            "Configuration file must have .yaml, .yml, or .json extension"
        )),
    }
}

/// Load soul templates from a directory.
///
/// Reads all `*.md` files from the given directory and maps filename (without extension)
/// to file content. This is used to load role-specific prompts for state machine agents.
///
/// # Example
///
/// ```ignore
/// let templates = load_soul_templates("~/.nanobot/soul_templates");
/// // Returns: {"taizi": "...", "zhongshu": "...", ...}
/// ```
pub fn load_soul_templates(dir: &Path) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    let mut templates = HashMap::new();

    if !dir.exists() {
        return templates;
    }

    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "md") {
                    if let Some(stem) = path.file_stem() {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            templates.insert(stem.to_string_lossy().to_string(), content);
                        }
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to read soul templates directory: {}", e);
        }
    }

    templates
}

/// Builder for creating StateMachineConfig programmatically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateMachineConfigBuilder {
    #[serde(default)]
    pub initial_state: Option<String>,
    #[serde(default)]
    pub terminal_states: Vec<String>,
    #[serde(default)]
    pub active_states: Vec<String>,
    #[serde(default)]
    pub sync_roles: Vec<String>,
    #[serde(default)]
    pub gates: std::collections::HashMap<String, GateConfig>,
    #[serde(default)]
    pub transitions: Vec<Transition>,
    #[serde(default)]
    pub state_roles: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub roles: std::collections::HashMap<String, AgentRoleConfig>,
    #[serde(default)]
    pub max_reviews: Option<u32>,
    #[serde(default)]
    pub stall_timeout_secs: Option<u64>,
}

impl StateMachineConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initial_state(mut self, state: &str) -> Self {
        self.initial_state = Some(state.to_string());
        self
    }

    pub fn terminal_states(mut self, states: Vec<String>) -> Self {
        self.terminal_states = states;
        self
    }

    pub fn add_terminal_state(mut self, state: &str) -> Self {
        self.terminal_states.push(state.to_string());
        self
    }

    pub fn active_states(mut self, states: Vec<String>) -> Self {
        self.active_states = states;
        self
    }

    pub fn add_active_state(mut self, state: &str) -> Self {
        self.active_states.push(state.to_string());
        self
    }

    pub fn sync_roles(mut self, roles: Vec<String>) -> Self {
        self.sync_roles = roles;
        self
    }

    pub fn add_sync_role(mut self, role: &str) -> Self {
        self.sync_roles.push(role.to_string());
        self
    }

    pub fn transition(mut self, from: &str, to: &str) -> Self {
        self.transitions.push(Transition {
            from: from.to_string(),
            to: to.to_string(),
            guard: None,
            action: None,
        });
        self
    }

    pub fn state_role(mut self, state: &str, role: &str) -> Self {
        self.state_roles.insert(state.to_string(), role.to_string());
        self
    }

    pub fn gate(mut self, state: &str, reject_to: &str) -> Self {
        self.gates.insert(
            state.to_string(),
            GateConfig {
                reject_to: reject_to.to_string(),
            },
        );
        self
    }

    pub fn max_reviews(mut self, max: u32) -> Self {
        self.max_reviews = Some(max);
        self
    }

    pub fn stall_timeout_secs(mut self, timeout: u64) -> Self {
        self.stall_timeout_secs = Some(timeout);
        self
    }

    pub fn build(self) -> anyhow::Result<StateMachineConfig> {
        let default = StateMachineConfig::default();

        let config = StateMachineConfig {
            initial_state: self.initial_state.unwrap_or(default.initial_state),
            terminal_states: self.terminal_states.into_iter().collect(),
            active_states: self.active_states.into_iter().collect(),
            sync_roles: self.sync_roles.into_iter().collect(),
            gates: self.gates,
            transitions: self.transitions,
            state_roles: self.state_roles,
            roles: self.roles,
            max_reviews: self.max_reviews.unwrap_or(default.max_reviews),
            stall_timeout_secs: self
                .stall_timeout_secs
                .unwrap_or(default.stall_timeout_secs),
        };

        config.validate().map_err(|errors| {
            anyhow::anyhow!("Validation failed:\n  - {}", errors.join("\n  - "))
        })?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_minimal() {
        let config = StateMachineConfigBuilder::new()
            .initial_state("analysis")
            .add_terminal_state("done")
            .transition("pending", "analysis")
            .transition("analysis", "done")
            .state_role("analysis", "analyst")
            .state_role("pending", "system")
            .state_role("done", "system")
            .build()
            .unwrap();

        assert_eq!(config.initial_state, "analysis");
        assert!(config.is_terminal("done"));
        assert!(config.can_transition("pending", "analysis"));
        assert!(config.can_transition("analysis", "done"));
    }

    #[test]
    fn test_builder_validation_failure() {
        // Missing transitions - initial state not in graph
        let result = StateMachineConfigBuilder::new()
            .initial_state("nonexistent")
            .build();

        assert!(result.is_err());
    }
}
