//! Phase enum and transition validation for the phased agent loop.

use std::fmt;

/// Phases of the phased agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentPhase {
    Research,
    Planning,
    Execute,
    Review,
    Done,
}

impl AgentPhase {
    /// Returns the string representation of this phase.
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentPhase::Research => "research",
            AgentPhase::Planning => "planning",
            AgentPhase::Execute => "execute",
            AgentPhase::Review => "review",
            AgentPhase::Done => "done",
        }
    }

    /// Returns true if transitioning from `self` to `target` is valid.
    pub fn can_transition_to(&self, target: &AgentPhase) -> bool {
        matches!(
            (self, target),
            (AgentPhase::Research, AgentPhase::Planning)
                | (AgentPhase::Research, AgentPhase::Execute)
                | (AgentPhase::Planning, AgentPhase::Execute)
                | (AgentPhase::Execute, AgentPhase::Review)
                | (AgentPhase::Execute, AgentPhase::Done)
                | (AgentPhase::Review, AgentPhase::Done)
        )
    }

    /// Returns the hard maximum iterations allowed in this phase.
    pub fn max_iterations(&self) -> u32 {
        match self {
            AgentPhase::Research => 7,
            AgentPhase::Planning => 5,
            AgentPhase::Execute => u32::MAX,
            AgentPhase::Review => 5,
            AgentPhase::Done => 0,
        }
    }

    /// Returns the soft limit iterations for this phase.
    /// A soft limit of 0 means no soft limit applies.
    pub fn soft_limit_iterations(&self) -> u32 {
        match self {
            AgentPhase::Research => 5,
            AgentPhase::Planning => 3,
            AgentPhase::Execute => 0,
            AgentPhase::Review => 3,
            AgentPhase::Done => 0,
        }
    }

    /// Returns the forced transition target when the phase exceeds its
    /// iteration limit, if any.
    pub fn forced_transition_target(&self) -> Option<&AgentPhase> {
        match self {
            AgentPhase::Research | AgentPhase::Planning => Some(&AgentPhase::Execute),
            AgentPhase::Review => Some(&AgentPhase::Done),
            AgentPhase::Execute | AgentPhase::Done => None,
        }
    }

    /// Returns the list of tool names explicitly allowed in this phase.
    /// An empty slice means all tools are allowed.
    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            AgentPhase::Research => &[
                "wiki_search",
                "wiki_read",
                "history_search",
                "query_history",
                "phase_transition",
            ],
            AgentPhase::Planning => &[
                "create_plan",
                "phase_transition",
                "wiki_read",
                "wiki_search",
            ],
            AgentPhase::Execute => &[],
            AgentPhase::Review => &[
                "wiki_write",
                "wiki_delete",
                "wiki_read",
                "wiki_search",
                "evolution",
                "phase_transition",
            ],
            AgentPhase::Done => &[],
        }
    }
}

impl fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for AgentPhase {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "research" => Ok(AgentPhase::Research),
            "planning" => Ok(AgentPhase::Planning),
            "execute" => Ok(AgentPhase::Execute),
            "review" => Ok(AgentPhase::Review),
            "done" => Ok(AgentPhase::Done),
            other => Err(format!("unknown phase: {}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Valid transitions ---

    #[test]
    fn test_valid_transition_research_to_planning() {
        assert!(AgentPhase::Research.can_transition_to(&AgentPhase::Planning));
    }

    #[test]
    fn test_valid_transition_research_to_execute() {
        assert!(AgentPhase::Research.can_transition_to(&AgentPhase::Execute));
    }

    #[test]
    fn test_valid_transition_planning_to_execute() {
        assert!(AgentPhase::Planning.can_transition_to(&AgentPhase::Execute));
    }

    #[test]
    fn test_valid_transition_execute_to_review() {
        assert!(AgentPhase::Execute.can_transition_to(&AgentPhase::Review));
    }

    #[test]
    fn test_valid_transition_execute_to_done() {
        assert!(AgentPhase::Execute.can_transition_to(&AgentPhase::Done));
    }

    #[test]
    fn test_valid_transition_review_to_done() {
        assert!(AgentPhase::Review.can_transition_to(&AgentPhase::Done));
    }

    // --- Invalid transitions ---

    #[test]
    fn test_invalid_transition_research_to_review() {
        assert!(!AgentPhase::Research.can_transition_to(&AgentPhase::Review));
    }

    #[test]
    fn test_invalid_transition_research_to_done() {
        assert!(!AgentPhase::Research.can_transition_to(&AgentPhase::Done));
    }

    #[test]
    fn test_invalid_transition_planning_to_review() {
        assert!(!AgentPhase::Planning.can_transition_to(&AgentPhase::Review));
    }

    #[test]
    fn test_invalid_transition_done_to_anything() {
        assert!(!AgentPhase::Done.can_transition_to(&AgentPhase::Research));
        assert!(!AgentPhase::Done.can_transition_to(&AgentPhase::Planning));
        assert!(!AgentPhase::Done.can_transition_to(&AgentPhase::Execute));
        assert!(!AgentPhase::Done.can_transition_to(&AgentPhase::Review));
        assert!(!AgentPhase::Done.can_transition_to(&AgentPhase::Done));
    }

    // --- Hard limit iterations ---

    #[test]
    fn test_max_iterations() {
        assert_eq!(AgentPhase::Research.max_iterations(), 7);
        assert_eq!(AgentPhase::Planning.max_iterations(), 5);
        assert_eq!(AgentPhase::Execute.max_iterations(), u32::MAX);
        assert_eq!(AgentPhase::Review.max_iterations(), 5);
        assert_eq!(AgentPhase::Done.max_iterations(), 0);
    }

    // --- Soft limit iterations ---

    #[test]
    fn test_soft_limit_iterations() {
        assert_eq!(AgentPhase::Research.soft_limit_iterations(), 5);
        assert_eq!(AgentPhase::Planning.soft_limit_iterations(), 3);
        assert_eq!(AgentPhase::Execute.soft_limit_iterations(), 0);
        assert_eq!(AgentPhase::Review.soft_limit_iterations(), 3);
        assert_eq!(AgentPhase::Done.soft_limit_iterations(), 0);
    }

    // --- Forced transition targets ---

    #[test]
    fn test_forced_transition_targets() {
        assert_eq!(
            AgentPhase::Research.forced_transition_target(),
            Some(&AgentPhase::Execute)
        );
        assert_eq!(
            AgentPhase::Planning.forced_transition_target(),
            Some(&AgentPhase::Execute)
        );
        assert_eq!(
            AgentPhase::Review.forced_transition_target(),
            Some(&AgentPhase::Done)
        );
        assert_eq!(AgentPhase::Execute.forced_transition_target(), None);
        assert_eq!(AgentPhase::Done.forced_transition_target(), None);
    }

    // --- from_str roundtrip ---

    #[test]
    fn test_from_str_roundtrip() {
        for (phase, name) in [
            (AgentPhase::Research, "research"),
            (AgentPhase::Planning, "planning"),
            (AgentPhase::Execute, "execute"),
            (AgentPhase::Review, "review"),
            (AgentPhase::Done, "done"),
        ] {
            assert_eq!(AgentPhase::try_from(name), Ok(phase));
            assert_eq!(phase.as_str(), name);
            assert_eq!(phase.to_string(), name);
        }
        assert!(AgentPhase::try_from("invalid").is_err());
    }

    // --- allowed_tools ---

    #[test]
    fn test_allowed_tools_research() {
        let tools = AgentPhase::Research.allowed_tools();
        assert!(tools.contains(&"wiki_search"));
        assert!(tools.contains(&"wiki_read"));
        assert!(tools.contains(&"phase_transition"));
        assert!(!tools.contains(&"shell"));
    }

    #[test]
    fn test_allowed_tools_execute_empty() {
        let tools = AgentPhase::Execute.allowed_tools();
        assert!(tools.is_empty());
    }
}
