use super::agent_phase::AgentPhase;
use super::phase_prompt::ContextAccumulator;

/// Internal state machine for phase tracking.
pub struct PhaseStateMachine {
    phase: AgentPhase,
    iteration_in_phase: u32,
    total_iterations: u32,
    context: ContextAccumulator,
}

impl PhaseStateMachine {
    pub fn new() -> Self {
        Self {
            phase: AgentPhase::Research,
            iteration_in_phase: 0,
            total_iterations: 0,
            context: ContextAccumulator::new(),
        }
    }

    pub fn current_phase(&self) -> AgentPhase {
        self.phase
    }
    pub fn iteration_in_phase(&self) -> u32 {
        self.iteration_in_phase
    }
    pub fn total_iterations(&self) -> u32 {
        self.total_iterations
    }
    pub fn context(&self) -> &ContextAccumulator {
        &self.context
    }

    pub fn add_context(&mut self, summary: String) {
        self.context.add(self.phase, summary);
    }

    pub fn advance_iteration(&mut self) {
        self.iteration_in_phase += 1;
        self.total_iterations += 1;
    }

    pub fn transition(&mut self, target: AgentPhase) -> Result<(), String> {
        if !self.phase.can_transition_to(&target) {
            return Err(format!(
                "Invalid phase transition: {} -> {}",
                self.phase, target
            ));
        }
        self.phase = target;
        self.iteration_in_phase = 0;
        Ok(())
    }

    pub fn is_at_soft_limit(&self) -> bool {
        let soft = self.phase.soft_limit_iterations();
        soft > 0 && self.iteration_in_phase >= soft
    }

    pub fn is_at_hard_limit(&self) -> bool {
        let hard = self.phase.max_iterations();
        hard > 0 && hard < u32::MAX && self.iteration_in_phase >= hard
    }

    pub fn is_at_global_limit(&self, global_max: u32) -> bool {
        self.total_iterations >= global_max
    }

    pub fn force_transition(&mut self) -> Result<AgentPhase, String> {
        if let Some(target) = self.phase.forced_transition_target() {
            let target = *target;
            self.transition(target)?;
            Ok(target)
        } else {
            Err(format!("Cannot force-transition from {}", self.phase))
        }
    }
}

/// Main entry point for phased execution.
/// The full run() method will be completed when integrating with SteppableExecutor.
pub struct PhasedExecutor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_phase_is_research() {
        let sm = PhaseStateMachine::new();
        assert_eq!(sm.current_phase(), AgentPhase::Research);
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 0);
    }

    #[test]
    fn test_valid_transition() {
        let mut sm = PhaseStateMachine::new();
        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 0);
    }

    #[test]
    fn test_invalid_transition() {
        let mut sm = PhaseStateMachine::new();
        let result = sm.transition(AgentPhase::Review);
        assert!(result.is_err());
        assert_eq!(sm.current_phase(), AgentPhase::Research);
    }

    #[test]
    fn test_iteration_tracking() {
        let mut sm = PhaseStateMachine::new();
        sm.advance_iteration();
        sm.advance_iteration();
        assert_eq!(sm.iteration_in_phase(), 2);
        assert_eq!(sm.total_iterations(), 2);
        sm.transition(AgentPhase::Execute).unwrap();
        assert_eq!(sm.iteration_in_phase(), 0);
        assert_eq!(sm.total_iterations(), 2);
    }

    #[test]
    fn test_soft_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..5 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_soft_limit());
        assert!(!sm.is_at_hard_limit());
    }

    #[test]
    fn test_hard_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 {
            sm.advance_iteration();
        }
        assert!(sm.is_at_hard_limit());
    }

    #[test]
    fn test_force_transition() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..7 {
            sm.advance_iteration();
        }
        let target = sm.force_transition().unwrap();
        assert_eq!(target, AgentPhase::Execute);
        assert_eq!(sm.current_phase(), AgentPhase::Execute);
    }

    #[test]
    fn test_context_accumulation() {
        let mut sm = PhaseStateMachine::new();
        sm.add_context("Found wiki pages".into());
        sm.transition(AgentPhase::Execute).unwrap();
        sm.add_context("Executed plan".into());
        assert!(sm.context().format().contains("Found wiki pages"));
        assert!(sm.context().format().contains("Executed plan"));
    }

    #[test]
    fn test_global_limit() {
        let mut sm = PhaseStateMachine::new();
        for _ in 0..99 {
            sm.advance_iteration();
        }
        assert!(!sm.is_at_global_limit(100));
        sm.advance_iteration();
        assert!(sm.is_at_global_limit(100));
    }
}
