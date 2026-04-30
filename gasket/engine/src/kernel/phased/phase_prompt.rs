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
    pub fn entry_prompt(phase: AgentPhase, ctx: &ContextAccumulator) -> String {
        let ctx_block = ctx.format();
        let ctx_section = if ctx_block.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", ctx_block)
        };
        match phase {
            AgentPhase::Research => "[Phase: Research]\n\n\
                 你现在处于研究阶段。使用 wiki_search 和 wiki_read 搜索知识库，\
                 用 history_search 查找历史对话。\n\
                 信息充分后向用户总结发现的内容。"
                .to_string(),
            AgentPhase::Planning => format!(
                "[Phase: Planning]\n\n\
                 {ctx_section}\
                 你现在处于计划阶段。基于以上信息和用户的需求，判断是否有足够的信息来制定计划。\n\n\
                 如果信息不足（目标不清晰、缺少关键上下文、用户意图模糊），请直接问用户澄清问题，不要调用 create_plan。\n\
                 只有在信息充分、目标明确时，才调用 create_plan 生成执行计划。\n\
                 计划应包含步骤、依赖和预期结果。"
            ),
            AgentPhase::Execute => format!(
                "[Phase: Execute]\n\n\
                 {ctx_section}\
                 执行你的计划。所有工具现在可用。"
            ),
            AgentPhase::Review => format!(
                "[Phase: Review]\n\n\
                 {ctx_section}\
                 审视执行过程：\n\
                 1. 结果是否达到了用户的目标？\n\
                 2. 有哪些值得持久保存的知识？\n\
                 3. 有哪些 wiki 页面应该创建或更新？\n\n\
                 如发现持久知识，主动调用 wiki_write 写入（每次最多 3 条）。"
            ),
            AgentPhase::Done => String::new(),
        }
    }

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
    fn test_research_entry_prompt() {
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Research, &ContextAccumulator::new());
        assert!(prompt.contains("Research"));
        assert!(prompt.contains("wiki_search"));
        // Research prompt should NOT contain phase_transition (user drives transitions)
        assert!(!prompt.contains("phase_transition"));
    }

    #[test]
    fn test_planning_with_research_summary() {
        let mut ctx = ContextAccumulator::new();
        ctx.add(AgentPhase::Research, "Found 3 wiki pages".into());
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Planning, &ctx);
        assert!(prompt.contains("Planning"));
        assert!(prompt.contains("Found 3 wiki pages"));
        assert!(!prompt.contains("phase_transition"));
    }

    #[test]
    fn test_execute_with_accumulated_context() {
        let mut ctx = ContextAccumulator::new();
        ctx.add(AgentPhase::Research, "User wants async info".into());
        ctx.add(AgentPhase::Planning, "Plan: read docs".into());
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Execute, &ctx);
        assert!(prompt.contains("Execute"));
        assert!(prompt.contains("User wants async info"));
        assert!(prompt.contains("Plan: read docs"));
    }

    #[test]
    fn test_review_entry_prompt() {
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Review, &ContextAccumulator::new());
        assert!(prompt.contains("Review"));
        assert!(prompt.contains("wiki_write"));
    }

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
