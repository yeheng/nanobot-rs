//! Agent role classification.
//!
//! Distinguishes whether a `RuntimeContext` belongs to the Orchestrator
//! (main agent, can dispatch workers) or a Worker (subagent, leaf node).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Worker,
}

impl AgentRole {
    /// Only the Orchestrator can spawn workers.
    pub fn can_spawn(&self) -> bool {
        matches!(self, AgentRole::Orchestrator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_can_spawn() {
        assert!(AgentRole::Orchestrator.can_spawn());
    }

    #[test]
    fn worker_cannot_spawn() {
        assert!(!AgentRole::Worker.can_spawn());
    }

    #[test]
    fn role_is_copy() {
        let r = AgentRole::Orchestrator;
        let r2 = r;
        assert_eq!(r, r2);
    }
}
