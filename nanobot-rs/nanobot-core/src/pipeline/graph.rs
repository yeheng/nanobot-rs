//! Data-driven pipeline graph replacing the hard-coded `TaskState` enum.
//!
//! `PipelineGraph` holds all routing logic — states, transitions, role mappings,
//! sync/async dispatch classification, and gate configurations — as runtime data.
//! This allows arbitrary pipeline topologies to be defined via configuration
//! while `default_sansheng()` provides backward-compatible defaults.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Gate configuration for states that require review-count enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    /// State to transition to when the review limit is exceeded.
    #[serde(alias = "rejectTo")]
    pub reject_to: String,
}

/// A data-driven directed graph that defines the pipeline topology.
///
/// Replaces the former `TaskState` enum — every piece of information that was
/// hard-coded in match arms is now stored as runtime data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineGraph {
    /// The state a task transitions to immediately after creation.
    #[serde(alias = "entryState")]
    pub entry_state: String,

    /// States that signify completion (no further dispatch needed).
    #[serde(alias = "terminalStates")]
    pub terminal_states: HashSet<String>,

    /// States considered "active" for stall detection.
    #[serde(alias = "activeStates")]
    pub active_states: HashSet<String>,

    /// Roles that use synchronous (wait) dispatch.
    #[serde(alias = "syncRoles")]
    pub sync_roles: HashSet<String>,

    /// Gate configurations keyed by state name.
    #[serde(default)]
    pub gates: HashMap<String, GateConfig>,

    /// Transition table: source state → list of allowed target states.
    pub transitions: HashMap<String, Vec<String>>,

    /// State → responsible role mapping.
    #[serde(alias = "stateRoles")]
    pub state_roles: HashMap<String, String>,
}

impl PipelineGraph {
    /// Produce the default graph matching the original 三省六部 state machine.
    ///
    /// This is a 1:1 port of the former `TaskState` enum:
    /// ```text
    /// Pending → Triage → Planning → Reviewing ─┬→ Assigned → Executing ─┬→ Review → Done
    ///                                           └→ Planning (reject)     └→ Blocked
    /// Blocked → Executing | Planning
    /// ```
    pub fn default_sansheng() -> Self {
        let mut transitions = HashMap::new();
        transitions.insert("pending".into(), vec!["triage".into()]);
        transitions.insert("triage".into(), vec!["planning".into()]);
        transitions.insert("planning".into(), vec!["reviewing".into()]);
        transitions.insert(
            "reviewing".into(),
            vec!["assigned".into(), "planning".into()],
        );
        transitions.insert("assigned".into(), vec!["executing".into()]);
        transitions.insert("executing".into(), vec!["review".into(), "blocked".into()]);
        transitions.insert("review".into(), vec!["done".into(), "blocked".into()]);
        transitions.insert("done".into(), vec![]);
        transitions.insert(
            "blocked".into(),
            vec!["executing".into(), "planning".into()],
        );

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
            entry_state: "triage".into(),
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
        }
    }

    /// Validate the graph for internal consistency.
    ///
    /// Checks:
    /// 1. entry_state exists in the transition table
    /// 2. All terminal states exist in the transition table
    /// 3. All active states exist in the transition table
    /// 4. All transition targets exist in the transition table
    /// 5. All states in state_roles exist in the transition table
    /// 6. All gate states exist in the transition table
    /// 7. All gate reject_to targets exist in the transition table
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let all_states: HashSet<&str> = self.transitions.keys().map(|s| s.as_str()).collect();

        // 1. entry_state must be a known state
        if !all_states.contains(self.entry_state.as_str()) {
            errors.push(format!(
                "entry_state '{}' is not defined in transitions",
                self.entry_state
            ));
        }

        // 2. terminal_states must be known
        for ts in &self.terminal_states {
            if !all_states.contains(ts.as_str()) {
                errors.push(format!(
                    "terminal_state '{}' is not defined in transitions",
                    ts
                ));
            }
        }

        // 3. active_states must be known
        for s in &self.active_states {
            if !all_states.contains(s.as_str()) {
                errors.push(format!(
                    "active_state '{}' is not defined in transitions",
                    s
                ));
            }
        }

        // 4. All transition targets must be known states
        for (from, targets) in &self.transitions {
            for to in targets {
                if !all_states.contains(to.as_str()) {
                    errors.push(format!(
                        "transition target '{}' (from '{}') is not defined in transitions",
                        to, from
                    ));
                }
            }
        }

        // 5. state_roles must reference known states
        for state in self.state_roles.keys() {
            if !all_states.contains(state.as_str()) {
                errors.push(format!(
                    "state_roles key '{}' is not defined in transitions",
                    state
                ));
            }
        }

        // 6 & 7. gate states and reject_to targets must be known
        for (state, gate) in &self.gates {
            if !all_states.contains(state.as_str()) {
                errors.push(format!(
                    "gate state '{}' is not defined in transitions",
                    state
                ));
            }
            if !all_states.contains(gate.reject_to.as_str()) {
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

    /// Check if a state is known to this graph.
    pub fn is_valid_state(&self, state: &str) -> bool {
        self.transitions.contains_key(state)
    }

    /// Check whether transitioning from `from` to `to` is legal.
    pub fn can_transition(&self, from: &str, to: &str) -> bool {
        self.transitions
            .get(from)
            .map(|targets| targets.iter().any(|t| t == to))
            .unwrap_or(false)
    }

    /// Return the list of allowed target states from `from`.
    pub fn allowed_transitions(&self, from: &str) -> &[String] {
        self.transitions
            .get(from)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return the responsible role for a given state.
    pub fn responsible_role(&self, state: &str) -> Option<&str> {
        self.state_roles.get(state).map(|s| s.as_str())
    }

    /// Check whether a role uses synchronous dispatch.
    pub fn is_sync_role(&self, role: &str) -> bool {
        self.sync_roles.contains(role)
    }

    /// Return the gate configuration for a state, if any.
    pub fn gate_config(&self, state: &str) -> Option<&GateConfig> {
        self.gates.get(state)
    }

    /// Check whether a state is terminal (no further dispatch).
    pub fn is_terminal(&self, state: &str) -> bool {
        self.terminal_states.contains(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_sansheng_validates() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn test_legal_transitions() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.can_transition("pending", "triage"));
        assert!(graph.can_transition("triage", "planning"));
        assert!(graph.can_transition("planning", "reviewing"));
        assert!(graph.can_transition("reviewing", "assigned"));
        assert!(graph.can_transition("reviewing", "planning"));
        assert!(graph.can_transition("assigned", "executing"));
        assert!(graph.can_transition("executing", "review"));
        assert!(graph.can_transition("executing", "blocked"));
        assert!(graph.can_transition("review", "done"));
        assert!(graph.can_transition("review", "blocked"));
        assert!(graph.can_transition("blocked", "executing"));
        assert!(graph.can_transition("blocked", "planning"));
    }

    #[test]
    fn test_illegal_transitions() {
        let graph = PipelineGraph::default_sansheng();
        assert!(!graph.can_transition("pending", "done"));
        assert!(!graph.can_transition("triage", "executing"));
        assert!(!graph.can_transition("done", "pending"));
        assert!(!graph.can_transition("executing", "planning"));
    }

    #[test]
    fn test_responsible_roles() {
        let graph = PipelineGraph::default_sansheng();
        assert_eq!(graph.responsible_role("triage"), Some("taizi"));
        assert_eq!(graph.responsible_role("planning"), Some("zhongshu"));
        assert_eq!(graph.responsible_role("reviewing"), Some("menxia"));
        assert_eq!(graph.responsible_role("assigned"), Some("shangshu"));
        assert_eq!(graph.responsible_role("executing"), Some("ministry"));
        assert_eq!(graph.responsible_role("review"), Some("menxia"));
        assert_eq!(graph.responsible_role("done"), Some("system"));
        assert_eq!(graph.responsible_role("blocked"), Some("shangshu"));
    }

    #[test]
    fn test_is_valid_state() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.is_valid_state("pending"));
        assert!(graph.is_valid_state("triage"));
        assert!(graph.is_valid_state("done"));
        assert!(!graph.is_valid_state("unknown"));
        assert!(!graph.is_valid_state(""));
    }

    #[test]
    fn test_terminal_states() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.is_terminal("done"));
        assert!(!graph.is_terminal("pending"));
        assert!(!graph.is_terminal("executing"));
    }

    #[test]
    fn test_sync_roles() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.is_sync_role("taizi"));
        assert!(graph.is_sync_role("zhongshu"));
        assert!(graph.is_sync_role("menxia"));
        assert!(graph.is_sync_role("shangshu"));
        assert!(!graph.is_sync_role("ministry"));
        assert!(!graph.is_sync_role("gong"));
    }

    #[test]
    fn test_gate_config() {
        let graph = PipelineGraph::default_sansheng();
        let gate = graph
            .gate_config("reviewing")
            .expect("reviewing has a gate");
        assert_eq!(gate.reject_to, "blocked");
        assert!(graph.gate_config("planning").is_none());
    }

    #[test]
    fn test_active_states() {
        let graph = PipelineGraph::default_sansheng();
        assert!(graph.active_states.contains("executing"));
        assert!(graph.active_states.contains("triage"));
        assert!(graph.active_states.contains("planning"));
        assert!(graph.active_states.contains("reviewing"));
        assert!(graph.active_states.contains("assigned"));
        assert!(!graph.active_states.contains("pending"));
        assert!(!graph.active_states.contains("done"));
        assert!(!graph.active_states.contains("blocked"));
    }

    #[test]
    fn test_validate_catches_invalid_entry_state() {
        let mut graph = PipelineGraph::default_sansheng();
        graph.entry_state = "nonexistent".into();
        let errors = graph.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("entry_state")));
    }

    #[test]
    fn test_validate_catches_invalid_terminal_state() {
        let mut graph = PipelineGraph::default_sansheng();
        graph.terminal_states.insert("fantasy".into());
        let errors = graph.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("terminal_state")));
    }

    #[test]
    fn test_validate_catches_invalid_transition_target() {
        let mut graph = PipelineGraph::default_sansheng();
        graph
            .transitions
            .get_mut("pending")
            .unwrap()
            .push("nowhere".into());
        let errors = graph.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("transition target")));
    }

    #[test]
    fn test_validate_catches_invalid_gate_reject_to() {
        let mut graph = PipelineGraph::default_sansheng();
        graph.gates.insert(
            "reviewing".into(),
            GateConfig {
                reject_to: "void".into(),
            },
        );
        let errors = graph.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("gate reject_to")));
    }

    #[test]
    fn test_custom_graph() {
        let mut transitions = HashMap::new();
        transitions.insert("pending".into(), vec!["analysis".into()]);
        transitions.insert("analysis".into(), vec!["development".into()]);
        transitions.insert("development".into(), vec!["code_review".into()]);
        transitions.insert(
            "code_review".into(),
            vec!["testing".into(), "development".into()],
        );
        transitions.insert("testing".into(), vec!["done".into(), "blocked".into()]);
        transitions.insert(
            "blocked".into(),
            vec!["development".into(), "analysis".into()],
        );
        transitions.insert("done".into(), vec![]);

        let mut state_roles = HashMap::new();
        state_roles.insert("analysis".into(), "lead".into());
        state_roles.insert("development".into(), "developer".into());
        state_roles.insert("code_review".into(), "reviewer".into());
        state_roles.insert("testing".into(), "tester".into());
        state_roles.insert("blocked".into(), "lead".into());

        let mut gates = HashMap::new();
        gates.insert(
            "code_review".into(),
            GateConfig {
                reject_to: "development".into(),
            },
        );

        let graph = PipelineGraph {
            entry_state: "analysis".into(),
            terminal_states: HashSet::from(["done".into()]),
            active_states: HashSet::from([
                "analysis".into(),
                "development".into(),
                "testing".into(),
            ]),
            sync_roles: HashSet::from(["lead".into(), "reviewer".into()]),
            gates,
            transitions,
            state_roles,
        };

        assert!(graph.validate().is_ok());
        assert!(graph.can_transition("development", "code_review"));
        assert!(graph.can_transition("code_review", "development"));
        assert!(!graph.can_transition("analysis", "done"));
        assert_eq!(graph.responsible_role("analysis"), Some("lead"));
        assert!(graph.is_sync_role("lead"));
        assert!(!graph.is_sync_role("developer"));
    }
}
