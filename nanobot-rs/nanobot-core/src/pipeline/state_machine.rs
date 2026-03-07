//! Task state machine for the multi-agent pipeline.
//!
//! Defines `TaskState` and the legal transitions between states.
//! Each state maps to a responsible role so the orchestrator knows
//! which agent to dispatch after every transition.

use serde::{Deserialize, Serialize};
use std::fmt;

/// All possible states a pipeline task can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Newly created, waiting to enter the pipeline.
    Pending,
    /// Being analyzed and classified by the Triage agent (太子).
    Triage,
    /// Under strategic planning by Zhongshu (中书省).
    Planning,
    /// Under review / quality gate by Menxia (门下省).
    Reviewing,
    /// Approved and assigned to an execution ministry.
    Assigned,
    /// Currently being executed by a ministry agent (六部).
    Executing,
    /// Post-execution review.
    Review,
    /// Successfully completed.
    Done,
    /// Blocked — waiting for external resolution or recovery.
    Blocked,
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskState::Pending => write!(f, "pending"),
            TaskState::Triage => write!(f, "triage"),
            TaskState::Planning => write!(f, "planning"),
            TaskState::Reviewing => write!(f, "reviewing"),
            TaskState::Assigned => write!(f, "assigned"),
            TaskState::Executing => write!(f, "executing"),
            TaskState::Review => write!(f, "review"),
            TaskState::Done => write!(f, "done"),
            TaskState::Blocked => write!(f, "blocked"),
        }
    }
}

impl TaskState {
    /// Parse a state from its string representation.
    pub fn from_str_lossy(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "triage" => Some(Self::Triage),
            "planning" => Some(Self::Planning),
            "reviewing" => Some(Self::Reviewing),
            "assigned" => Some(Self::Assigned),
            "executing" => Some(Self::Executing),
            "review" => Some(Self::Review),
            "done" => Some(Self::Done),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    /// Return the set of states this state is allowed to transition **to**.
    pub fn allowed_transitions(&self) -> &'static [TaskState] {
        match self {
            TaskState::Pending => &[TaskState::Triage],
            TaskState::Triage => &[TaskState::Planning],
            TaskState::Planning => &[TaskState::Reviewing],
            // Menxia can approve (→ Assigned) or reject (→ Planning)
            TaskState::Reviewing => &[TaskState::Assigned, TaskState::Planning],
            TaskState::Assigned => &[TaskState::Executing],
            TaskState::Executing => &[TaskState::Review, TaskState::Blocked],
            TaskState::Review => &[TaskState::Done, TaskState::Blocked],
            TaskState::Done => &[],
            // Recovery paths from Blocked
            TaskState::Blocked => &[TaskState::Executing, TaskState::Planning],
        }
    }

    /// Check whether transitioning from `self` to `target` is legal.
    pub fn can_transition_to(&self, target: TaskState) -> bool {
        self.allowed_transitions().contains(&target)
    }

    /// Return the default responsible role for this state.
    ///
    /// The orchestrator uses this to decide which agent to dispatch.
    pub fn responsible_role(&self) -> &'static str {
        match self {
            TaskState::Pending => "system",
            TaskState::Triage => "taizi",
            TaskState::Planning => "zhongshu",
            TaskState::Reviewing => "menxia",
            TaskState::Assigned => "shangshu",
            TaskState::Executing => "ministry", // resolved at runtime
            TaskState::Review => "menxia",
            TaskState::Done => "system",
            TaskState::Blocked => "shangshu",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legal_transitions() {
        assert!(TaskState::Pending.can_transition_to(TaskState::Triage));
        assert!(TaskState::Triage.can_transition_to(TaskState::Planning));
        assert!(TaskState::Planning.can_transition_to(TaskState::Reviewing));
        assert!(TaskState::Reviewing.can_transition_to(TaskState::Assigned));
        assert!(TaskState::Reviewing.can_transition_to(TaskState::Planning));
        assert!(TaskState::Assigned.can_transition_to(TaskState::Executing));
        assert!(TaskState::Executing.can_transition_to(TaskState::Review));
        assert!(TaskState::Executing.can_transition_to(TaskState::Blocked));
        assert!(TaskState::Review.can_transition_to(TaskState::Done));
        assert!(TaskState::Review.can_transition_to(TaskState::Blocked));
        assert!(TaskState::Blocked.can_transition_to(TaskState::Executing));
        assert!(TaskState::Blocked.can_transition_to(TaskState::Planning));
    }

    #[test]
    fn test_illegal_transitions() {
        assert!(!TaskState::Pending.can_transition_to(TaskState::Done));
        assert!(!TaskState::Triage.can_transition_to(TaskState::Executing));
        assert!(!TaskState::Done.can_transition_to(TaskState::Pending));
        assert!(!TaskState::Executing.can_transition_to(TaskState::Planning));
    }

    #[test]
    fn test_display_roundtrip() {
        let states = [
            TaskState::Pending,
            TaskState::Triage,
            TaskState::Planning,
            TaskState::Reviewing,
            TaskState::Assigned,
            TaskState::Executing,
            TaskState::Review,
            TaskState::Done,
            TaskState::Blocked,
        ];
        for state in states {
            let s = state.to_string();
            assert_eq!(TaskState::from_str_lossy(&s), Some(state));
        }
    }

    #[test]
    fn test_responsible_roles() {
        assert_eq!(TaskState::Triage.responsible_role(), "taizi");
        assert_eq!(TaskState::Planning.responsible_role(), "zhongshu");
        assert_eq!(TaskState::Reviewing.responsible_role(), "menxia");
        assert_eq!(TaskState::Assigned.responsible_role(), "shangshu");
    }
}
