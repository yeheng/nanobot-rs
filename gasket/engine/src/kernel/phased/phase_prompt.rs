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
                 用 history_search 查找历史对话。\n\n\
                 你的工作流程：\n\
                 1. 搜索并收集相关信息\n\
                 2. 如果信息不足或需要澄清，直接向用户提问（AI 会暂停等你回复）\n\
                 3. 如果信息充分，向用户总结发现的内容\n\
                 4. 当你认为已经收集了足够信息，调用 phase_transition(\"planning\") 进入计划阶段，\
                 或 phase_transition(\"execute\") 直接跳过计划阶段。"
                .to_string(),
            AgentPhase::Planning => format!(
                "[Phase: Planning]\n\n\
                 {ctx_section}\
                 你现在处于计划阶段。基于以上信息和用户的需求，制定详细的执行计划。\n\n\
                 你的工作流程：\n\
                 1. 如果信息不足（目标不清晰、缺少关键上下文、用户意图模糊），请直接向用户提问澄清， 至少两三个问题.\n\
                 2. 信息充分时，直接输出结构化的执行计划（使用标题、步骤、依赖关系）\n\
                 3. 使用 write_file 工具将计划保存到 topics/plans/<slug> 路径下\n\
                 4. 计划输出并保存后，必须立即调用 phase_transition(\"execute\") 进入执行阶段，不要等待用户确认或询问是否开始执行"
            ),
            AgentPhase::Execute => format!(
                "[Phase: Execute]\n\n\
                 {ctx_section}\
                 执行你的计划。所有工具现在可用。\n\n\
                 你的工作流程：\n\
                 1. 按照计划逐步执行\n\
                 2. 如果需要向用户汇报进度或请示，直接输出文本（AI 会暂停等你回复）\n\
                 3. 执行完成后，调用 phase_transition(\"review\") 进入审查阶段"
            ),
            AgentPhase::Review => format!(
                "[Phase: Review]\n\n\
                 {ctx_section}\
                 审视执行过程：\n\
                 1. 结果是否达到了用户的目标？\n\
                 2. 有哪些值得持久保存的知识？\n\
                 3. 有哪些 wiki 页面应该创建或更新？\n\n\
                 你的工作流程：\n\
                 1. 审查结果，必要时调用 wiki_write 写入知识（每次最多 3 条）\n\
                 2. 如果发现问题需要修正，调用 phase_transition(\"planning\") 重新规划\
                 或 phase_transition(\"execute\") 补充执行\n\
                 3. 审查通过且无遗留问题后，调用 phase_transition(\"done\") 结束任务并输出本次任务的总结和经验教训"
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
        // Research prompt should guide LLM to call phase_transition when ready
        assert!(prompt.contains("phase_transition"));
    }

    #[test]
    fn test_planning_with_research_summary() {
        let mut ctx = ContextAccumulator::new();
        ctx.add(AgentPhase::Research, "Found 3 wiki pages".into());
        let prompt = PhasePrompt::entry_prompt(AgentPhase::Planning, &ctx);
        assert!(prompt.contains("Planning"));
        assert!(prompt.contains("Found 3 wiki pages"));
        // Planning prompt should guide LLM to call phase_transition to execute
        assert!(prompt.contains("phase_transition"));
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
