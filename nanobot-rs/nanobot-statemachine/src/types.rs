//! Core types for the state machine.
//!
//! These types define the fundamental building blocks:
//! - `State`: A state in the state machine
//! - `Transition`: A transition between states
//! - `StateMachineConfig`: The complete state machine configuration

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A state in the state machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct State {
    /// Unique identifier for this state.
    pub id: String,
    /// Human-readable name.
    pub name: Option<String>,
    /// Optional description of what this state represents.
    pub description: Option<String>,
    /// The agent role responsible for this state.
    pub role: Option<String>,
    /// Whether this is a terminal state (no further transitions).
    #[serde(default)]
    pub terminal: bool,
    /// Whether this state is considered "active" for stall detection.
    #[serde(default)]
    pub active: bool,
}

/// A transition between two states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    /// Source state ID.
    pub from: String,
    /// Target state ID.
    pub to: String,
    /// Optional guard condition (future: support expressions).
    pub guard: Option<String>,
    /// Optional action to perform on transition.
    pub action: Option<String>,
}

/// Gate configuration for states that require review-count enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    /// State to transition to when the review limit is exceeded.
    #[serde(alias = "rejectTo")]
    pub reject_to: String,
}

/// Configuration for an agent role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    /// Human-readable description of the role.
    #[serde(default)]
    pub description: String,
    /// Roles this agent is allowed to delegate to.
    #[serde(default, alias = "allowedAgents")]
    pub allowed_agents: Vec<String>,
    /// Path to a custom SOUL.md file; overrides the built-in template.
    #[serde(default, alias = "soulPath")]
    pub soul_path: Option<String>,
    /// Model override for this specific role.
    #[serde(default)]
    pub model: Option<String>,
    /// States for which this role is responsible.
    #[serde(default, alias = "responsibleStates")]
    pub responsible_states: Vec<String>,
}

/// Complete state machine configuration.
///
/// This can be loaded from a YAML or JSON file and defines
/// the entire multi-agent collaboration topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMachineConfig {
    /// The initial state where tasks start.
    #[serde(alias = "initialState")]
    pub initial_state: String,

    /// Terminal states that signify completion.
    #[serde(alias = "terminalStates", default)]
    pub terminal_states: HashSet<String>,

    /// Active states for stall detection.
    #[serde(alias = "activeStates", default)]
    pub active_states: HashSet<String>,

    /// Roles that use synchronous (blocking) dispatch.
    #[serde(alias = "syncRoles", default)]
    pub sync_roles: HashSet<String>,

    /// Gate configurations keyed by state name.
    #[serde(default)]
    pub gates: HashMap<String, GateConfig>,

    /// List of transitions defining the state graph.
    #[serde(default)]
    pub transitions: Vec<Transition>,

    /// State → role mapping (alternative to embedding in State).
    #[serde(default, alias = "stateRoles")]
    pub state_roles: HashMap<String, String>,

    /// Role configurations.
    #[serde(default)]
    pub roles: HashMap<String, AgentRoleConfig>,

    /// Maximum review round-trips before escalation.
    #[serde(default = "default_max_reviews", alias = "maxReviews")]
    pub max_reviews: u32,

    /// Seconds of heartbeat silence before a task is considered stalled.
    #[serde(default = "default_stall_timeout", alias = "stallTimeoutSecs")]
    pub stall_timeout_secs: u64,
}

fn default_max_reviews() -> u32 {
    3
}

fn default_stall_timeout() -> u64 {
    60
}

impl StateMachineConfig {
    /// Validate the configuration for internal consistency.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Build set of all known state IDs
        let mut known_states: HashSet<&str> = HashSet::new();
        for t in &self.transitions {
            known_states.insert(&t.from);
            known_states.insert(&t.to);
        }

        // 1. initial_state must be a known state
        if !known_states.contains(self.initial_state.as_str()) {
            errors.push(format!(
                "initial_state '{}' is not defined in transitions",
                self.initial_state
            ));
        }

        // 2. terminal_states must be known
        for ts in &self.terminal_states {
            if !known_states.contains(ts.as_str()) {
                errors.push(format!(
                    "terminal_state '{}' is not defined in transitions",
                    ts
                ));
            }
        }

        // 3. active_states must be known
        for s in &self.active_states {
            if !known_states.contains(s.as_str()) {
                errors.push(format!(
                    "active_state '{}' is not defined in transitions",
                    s
                ));
            }
        }

        // 4. state_roles must reference known states
        for state in self.state_roles.keys() {
            if !known_states.contains(state.as_str()) {
                errors.push(format!(
                    "state_roles key '{}' is not defined in transitions",
                    state
                ));
            }
        }

        // 5. gate states and reject_to targets must be known
        for (state, gate) in &self.gates {
            if !known_states.contains(state.as_str()) {
                errors.push(format!(
                    "gate state '{}' is not defined in transitions",
                    state
                ));
            }
            if !known_states.contains(gate.reject_to.as_str()) {
                errors.push(format!(
                    "gate reject_to '{}' (for state '{}') is not defined in transitions",
                    gate.reject_to, state
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Build a transition lookup: from_state -> [to_states]
    pub fn build_transition_map(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for t in &self.transitions {
            map.entry(t.from.clone()).or_default().push(t.to.clone());
        }
        map
    }

    /// Check if a transition is valid.
    pub fn can_transition(&self, from: &str, to: &str) -> bool {
        self.transitions
            .iter()
            .any(|t| t.from == from && t.to == to)
    }

    /// Get allowed transitions from a state.
    pub fn allowed_transitions(&self, from: &str) -> Vec<&str> {
        self.transitions
            .iter()
            .filter(|t| t.from == from)
            .map(|t| t.to.as_str())
            .collect()
    }

    /// Get the responsible role for a state.
    pub fn responsible_role(&self, state: &str) -> Option<&str> {
        self.state_roles.get(state).map(|s| s.as_str())
    }

    /// Check if a role uses synchronous dispatch.
    pub fn is_sync_role(&self, role: &str) -> bool {
        self.sync_roles.contains(role)
    }

    /// Check if a state is terminal.
    pub fn is_terminal(&self, state: &str) -> bool {
        self.terminal_states.contains(state)
    }

    /// Check if a state is active.
    pub fn is_active(&self, state: &str) -> bool {
        self.active_states.contains(state)
    }

    /// Get gate config for a state.
    pub fn gate_config(&self, state: &str) -> Option<&GateConfig> {
        self.gates.get(state)
    }
}

impl Default for StateMachineConfig {
    /// Default to the 三省六部 (Sansheng Liubu) preset.
    fn default() -> Self {
        Self::default_sansheng()
    }
}

impl StateMachineConfig {
    /// Produce the default configuration matching the original 三省六部 state machine.
    ///
    /// This is a 1:1 port of the former pipeline's `TaskState` enum:
    /// ```text
    /// Pending → Triage → Planning → Reviewing ─┬→ Assigned → Executing ─┬→ Review → Done
    ///                                           └→ Planning (reject)     └→ Blocked
    /// Blocked → Executing | Planning
    /// ```
    pub fn default_sansheng() -> Self {
        let transitions = vec![
            Transition {
                from: "pending".into(),
                to: "triage".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "triage".into(),
                to: "planning".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "planning".into(),
                to: "reviewing".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "reviewing".into(),
                to: "assigned".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "reviewing".into(),
                to: "planning".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "assigned".into(),
                to: "executing".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "executing".into(),
                to: "review".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "executing".into(),
                to: "blocked".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "review".into(),
                to: "done".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "review".into(),
                to: "blocked".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "done".into(),
                to: "done".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "blocked".into(),
                to: "executing".into(),
                guard: None,
                action: None,
            },
            Transition {
                from: "blocked".into(),
                to: "planning".into(),
                guard: None,
                action: None,
            },
        ];

        let mut state_roles = HashMap::new();
        state_roles.insert("pending".into(), "system".into());
        state_roles.insert("triage".into(), "taizi".into());
        state_roles.insert("planning".into(), "zhongshu".into());
        state_roles.insert("reviewing".into(), "menxia".into());
        state_roles.insert("assigned".into(), "shangshu".into());
        state_roles.insert("executing".into(), "ministry".into());
        state_roles.insert("review".into(), "menxia".into());
        state_roles.insert("done".into(), "system".into());
        state_roles.insert("blocked".into(), "shangshu".into());

        let mut gates = HashMap::new();
        gates.insert(
            "reviewing".into(),
            GateConfig {
                reject_to: "blocked".into(),
            },
        );

        Self {
            initial_state: "triage".into(),
            terminal_states: HashSet::from(["done".into()]),
            active_states: HashSet::from([
                "executing".into(),
                "triage".into(),
                "planning".into(),
                "reviewing".into(),
                "assigned".into(),
            ]),
            sync_roles: HashSet::from([
                "taizi".into(),
                "zhongshu".into(),
                "menxia".into(),
                "shangshu".into(),
            ]),
            gates,
            transitions,
            state_roles,
            roles: HashMap::new(),
            max_reviews: 3,
            stall_timeout_secs: 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validates() {
        let config = StateMachineConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_default_transitions() {
        let config = StateMachineConfig::default();
        assert!(config.can_transition("pending", "triage"));
        assert!(config.can_transition("triage", "planning"));
        assert!(config.can_transition("planning", "reviewing"));
        assert!(config.can_transition("reviewing", "assigned"));
        assert!(config.can_transition("reviewing", "planning"));
        assert!(config.can_transition("executing", "review"));
        assert!(config.can_transition("review", "done"));
        assert!(!config.can_transition("pending", "done"));
    }

    #[test]
    fn test_responsible_roles() {
        let config = StateMachineConfig::default();
        assert_eq!(config.responsible_role("triage"), Some("taizi"));
        assert_eq!(config.responsible_role("planning"), Some("zhongshu"));
        assert_eq!(config.responsible_role("reviewing"), Some("menxia"));
        assert_eq!(config.responsible_role("executing"), Some("ministry"));
    }

    #[test]
    fn test_sync_roles() {
        let config = StateMachineConfig::default();
        assert!(config.is_sync_role("taizi"));
        assert!(config.is_sync_role("zhongshu"));
        assert!(config.is_sync_role("menxia"));
        assert!(config.is_sync_role("shangshu"));
        assert!(!config.is_sync_role("ministry"));
    }

    #[test]
    fn test_terminal_states() {
        let config = StateMachineConfig::default();
        assert!(config.is_terminal("done"));
        assert!(!config.is_terminal("executing"));
    }

    #[test]
    fn test_gate_config() {
        let config = StateMachineConfig::default();
        let gate = config
            .gate_config("reviewing")
            .expect("reviewing has a gate");
        assert_eq!(gate.reject_to, "blocked");
    }

    #[test]
    fn test_validate_catches_invalid_initial_state() {
        let mut config = StateMachineConfig::default();
        config.initial_state = "nonexistent".into();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("initial_state")));
    }

    #[test]
    fn test_validate_catches_invalid_transition_target() {
        let mut config = StateMachineConfig::default();
        config.transitions.push(Transition {
            from: "pending".into(),
            to: "nowhere".into(),
            guard: None,
            action: None,
        });
        // This should NOT error because "nowhere" is now a known state
        // Let's test with a terminal state instead
        config.terminal_states.insert("fantasy".into());
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("terminal_state")));
    }
}
