use std::collections::HashMap;

use super::agent_phase::AgentPhase;

#[derive(Debug, Default)]
pub struct ContextAccumulator {
    /// Stores the latest summary for each phase. When a phase is revisited
    /// (e.g. Review -> Execute -> Review loop), the old summary is overwritten
    /// to prevent context snowballing.
    summaries: HashMap<AgentPhase, String>,
}

impl ContextAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, phase: AgentPhase, summary: String) {
        self.summaries.insert(phase, summary);
    }

    pub fn format(&self) -> String {
        if self.summaries.is_empty() {
            return String::new();
        }
        let mut parts = vec!["## 已确定的前置上下文".to_string()];
        // Output in logical execution order so the LLM sees context chronologically
        for phase in [
            AgentPhase::Research,
            AgentPhase::Planning,
            AgentPhase::Execute,
            AgentPhase::Review,
        ] {
            if let Some(summary) = self.summaries.get(&phase) {
                parts.push(format!("### {} Phase\n{}", phase, summary));
            }
        }
        parts.join("\n\n")
    }
}

pub struct PhasePrompt;

impl PhasePrompt {
    pub fn soft_limit_prompt(phase: AgentPhase) -> String {
        format!(
            "[系统提示] {} 阶段已达到建议迭代上限。\
             信息已足够，请调用 phase_transition 进入下一阶段。",
            phase
        )
    }

    pub fn hard_limit_prompt(from: AgentPhase, to: AgentPhase) -> String {
        // Special harsh prompt when Review is force-closed to Done to prevent
        // the agent from silently shipping broken results.
        if matches!((from, to), (AgentPhase::Review, AgentPhase::Done)) {
            return format!(
                "[系统强制] {} 阶段达到迭代上限，强制结束任务。\
                 请立即向用户总结你失败的原因及当前进度。",
                from
            );
        }
        format!(
            "[系统强制] {} 阶段达到迭代上限，由引擎强制推进至 {} 阶段。",
            from, to
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_limit_prompt() {
        let prompt = PhasePrompt::soft_limit_prompt(AgentPhase::Research);
        assert!(prompt.contains("phase_transition"));
        assert!(prompt.contains("信息已足够"));
    }

    #[test]
    fn test_hard_limit_prompt() {
        let prompt = PhasePrompt::hard_limit_prompt(AgentPhase::Research, AgentPhase::Execute);
        assert!(prompt.contains("强制推进"));
        assert!(prompt.contains("execute"));
    }
}
