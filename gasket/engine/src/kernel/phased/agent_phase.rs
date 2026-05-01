//! Phase enum, transition validation, and self-contained phase definitions.
//!
//! Each phase is described by a `PhaseDefinition` that consolidates all rules
//! (entry prompt, tools, limits, transitions, gates, checklist) in one place.
//! The `default_definitions()` function returns definitions matching the
//! current hardcoded behavior — Step 3/4 of the refactor will switch the
//! controller to read from these instead of the enum methods.

use std::collections::HashMap;
use std::fmt;

use super::phase_prompt::ContextAccumulator;

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

// ── Phase Definition Types ─────────────────────────────────────────

/// Execution trace collected during a single phase run.
/// Feeds gate checks and checklist verification.
#[derive(Debug, Clone, Default)]
pub struct PhaseContext {
    pub tools_invoked: Vec<String>,
    pub files_written: Vec<String>,
    pub wiki_pages_written: Vec<String>,
    pub iterations: usize,
}

impl PhaseContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Result of a gate check.
#[derive(Debug, Clone)]
pub enum GateResult {
    Passed,
    Failed(String),
}

/// Trait for engine-enforced gate checks between phases.
pub trait GateCheck: Send + Sync {
    fn description(&self) -> &str;
    fn check(&self, ctx: &PhaseContext) -> GateResult;
}

/// A single checklist item for phase exit validation.
#[derive(Debug, Clone)]
pub struct ChecklistItem {
    pub label: String,
    pub auto_verify: Option<fn(&PhaseContext) -> bool>,
}

impl ChecklistItem {
    pub fn soft(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            auto_verify: None,
        }
    }

    pub fn verified(label: impl Into<String>, check: fn(&PhaseContext) -> bool) -> Self {
        Self {
            label: label.into(),
            auto_verify: Some(check),
        }
    }
}

/// Self-contained definition of a single phase.
///
/// All rules governing a phase live here — entry prompt, tools, limits,
/// transitions, gates, and exit checklist. Inspired by the superpowers
/// skill model where each skill is a self-contained unit.
pub struct PhaseDefinition {
    pub phase: AgentPhase,
    pub entry_prompt: fn(&ContextAccumulator) -> String,
    pub allowed_tools: fn() -> Vec<&'static str>,
    pub exit_checklist: Vec<ChecklistItem>,
    pub hard_gates: Vec<Box<dyn GateCheck>>,
    pub max_iterations: u32,
    pub soft_limit_iterations: u32,
    pub allowed_transitions: Vec<AgentPhase>,
    pub forced_transition_target: Option<AgentPhase>,
}

// ── Gate implementations ───────────────────────────────────────────

/// Gate that requires at least one file to have been written.
pub struct FilesWrittenGate;

impl GateCheck for FilesWrittenGate {
    fn description(&self) -> &str {
        "Planning phase must produce at least one file"
    }

    fn check(&self, ctx: &PhaseContext) -> GateResult {
        if ctx.files_written.is_empty() {
            GateResult::Failed(
                "计划阶段未产出任何文件。请使用 write_file 工具将计划保存后再尝试进入下一阶段。".into(),
            )
        } else {
            GateResult::Passed
        }
    }
}

// ── Entry prompt functions (extracted from PhasePrompt::entry_prompt) ──

fn research_entry_prompt(ctx: &ContextAccumulator) -> String {
    let ctx_block = ctx.format();
    let ctx_section = if ctx_block.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", ctx_block)
    };
    format!(
        "{ctx_section}\
         [Phase: Research]\n\n\
         你现在处于研究阶段。使用 wiki_search 和 wiki_read 搜索知识库，\
         用 history_search 查找历史对话。\n\n\
         你的工作流程：\n\
         1. 搜索并收集相关信息\n\
         2. 如果信息不足或需要澄清，直接向用户提问（AI 会暂停等你回复）\n\
         3. 如果信息充分，向用户总结发现的内容\n\
         4. 当你认为已经收集了足够信息，调用 phase_transition(\"planning\") 进入计划阶段，\
         或 phase_transition(\"execute\") 直接跳过计划阶段。"
    )
}

fn planning_entry_prompt(ctx: &ContextAccumulator) -> String {
    let ctx_block = ctx.format();
    let ctx_section = if ctx_block.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", ctx_block)
    };
    format!(
        "{ctx_section}\
         [Phase: Planning]\n\n\
         你现在处于计划阶段。基于以上信息和用户的需求，制定详细的执行计划。\n\n\
         你的工作流程：\n\
         1. 如果信息不足（目标不清晰、缺少关键上下文、用户意图模糊），请直接向用户提问澄清， 至少两三个问题.\n\
         2. 信息充分时，直接输出结构化的执行计划（使用标题、步骤、依赖关系）\n\
         3. 使用 write_file 工具将计划保存到 topics/plans/<slug> 路径下\n\
         4. 计划输出并保存后，必须立即调用 phase_transition(\"execute\") 进入执行阶段，不要等待用户确认或询问是否开始执行"
    )
}

fn execute_entry_prompt(ctx: &ContextAccumulator) -> String {
    let ctx_block = ctx.format();
    let ctx_section = if ctx_block.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", ctx_block)
    };
    format!(
        "{ctx_section}\
         [Phase: Execute]\n\n\
         执行你的计划。所有工具现在可用。\n\n\
         你的工作流程：\n\
         1. 按照计划逐步执行\n\
         2. 如果需要向用户汇报进度或请示，直接输出文本（AI 会暂停等你回复）\n\
         3. 执行完成后，调用 phase_transition(\"review\") 进入审查阶段"
    )
}

fn review_entry_prompt(ctx: &ContextAccumulator) -> String {
    let ctx_block = ctx.format();
    let ctx_section = if ctx_block.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", ctx_block)
    };
    format!(
        "{ctx_section}\
         [Phase: Review]\n\n\
         审视执行过程：\n\
         1. 结果是否达到了用户的目标？\n\
         2. 有哪些值得持久保存的知识？\n\
         3. 有哪些 wiki 页面应该创建或更新？\n\n\
         你的工作流程：\n\
         1. 审查结果，必要时调用 wiki_write 写入知识（每次最多 3 条）\n\
         2. 如果发现问题需要修正，调用 phase_transition(\"planning\") 重新规划\
         或 phase_transition(\"execute\") 补充执行\n\
         3. 审查通过且无遗留问题后，调用 phase_transition(\"done\") 结束任务并输出本次任务的总结和经验教训"
    )
}

fn done_entry_prompt(_ctx: &ContextAccumulator) -> String {
    String::new()
}

// ── Default definitions factory ────────────────────────────────────

/// Returns the default phase definitions matching the current hardcoded behavior.
pub fn default_definitions() -> HashMap<AgentPhase, PhaseDefinition> {
    let mut defs = HashMap::new();

    defs.insert(
        AgentPhase::Research,
        PhaseDefinition {
            phase: AgentPhase::Research,
            entry_prompt: research_entry_prompt,
            allowed_tools: || vec![
                "wiki_search",
                "wiki_read",
                "history_search",
                "query_history",
                "phase_transition",
            ],
            exit_checklist: vec![
                ChecklistItem::soft("信息已充分收集"),
                ChecklistItem::soft("已向用户总结发现"),
            ],
            hard_gates: vec![],
            max_iterations: 7,
            soft_limit_iterations: 5,
            allowed_transitions: vec![AgentPhase::Planning, AgentPhase::Execute],
            forced_transition_target: Some(AgentPhase::Execute),
        },
    );

    defs.insert(
        AgentPhase::Planning,
        PhaseDefinition {
            phase: AgentPhase::Planning,
            entry_prompt: planning_entry_prompt,
            allowed_tools: || vec!["wiki_write", "wiki_read", "wiki_search", "phase_transition"],
            exit_checklist: vec![
                ChecklistItem::verified("计划已保存为文件", |ctx| !ctx.files_written.is_empty()),
                ChecklistItem::soft("计划包含步骤列表"),
            ],
            hard_gates: vec![Box::new(FilesWrittenGate)],
            max_iterations: 5,
            soft_limit_iterations: 3,
            allowed_transitions: vec![AgentPhase::Execute],
            forced_transition_target: Some(AgentPhase::Execute),
        },
    );

    defs.insert(
        AgentPhase::Execute,
        PhaseDefinition {
            phase: AgentPhase::Execute,
            entry_prompt: execute_entry_prompt,
            allowed_tools: || vec![],
            exit_checklist: vec![
                ChecklistItem::soft("计划步骤已执行"),
                ChecklistItem::soft("产出物已验证"),
            ],
            hard_gates: vec![],
            max_iterations: u32::MAX,
            soft_limit_iterations: 0,
            allowed_transitions: vec![AgentPhase::Review, AgentPhase::Done],
            forced_transition_target: None,
        },
    );

    defs.insert(
        AgentPhase::Review,
        PhaseDefinition {
            phase: AgentPhase::Review,
            entry_prompt: review_entry_prompt,
            allowed_tools: || vec![
                "wiki_write",
                "wiki_delete",
                "wiki_read",
                "wiki_search",
                "evolution",
                "phase_transition",
            ],
            exit_checklist: vec![
                ChecklistItem::soft("结果达成目标"),
                ChecklistItem::soft("知识已持久化"),
            ],
            hard_gates: vec![],
            max_iterations: 5,
            soft_limit_iterations: 3,
            allowed_transitions: vec![
                AgentPhase::Planning,
                AgentPhase::Execute,
                AgentPhase::Done,
            ],
            forced_transition_target: Some(AgentPhase::Done),
        },
    );

    defs.insert(
        AgentPhase::Done,
        PhaseDefinition {
            phase: AgentPhase::Done,
            entry_prompt: done_entry_prompt,
            allowed_tools: || vec![],
            exit_checklist: vec![],
            hard_gates: vec![],
            max_iterations: 0,
            soft_limit_iterations: 0,
            allowed_transitions: vec![],
            forced_transition_target: None,
        },
    );

    defs
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- PhaseDefinition defaults ---

    #[test]
    fn test_default_definitions_cover_all_phases() {
        let defs = default_definitions();
        assert_eq!(defs.len(), 5);
        for phase in [
            AgentPhase::Research,
            AgentPhase::Planning,
            AgentPhase::Execute,
            AgentPhase::Review,
            AgentPhase::Done,
        ] {
            assert!(defs.contains_key(&phase), "missing definition for {phase}");
        }
    }

    #[test]
    fn test_default_definitions_limits() {
        let defs = default_definitions();
        assert_eq!(defs[&AgentPhase::Research].max_iterations, 7);
        assert_eq!(defs[&AgentPhase::Research].soft_limit_iterations, 5);
        assert_eq!(defs[&AgentPhase::Planning].max_iterations, 5);
        assert_eq!(defs[&AgentPhase::Planning].soft_limit_iterations, 3);
        assert_eq!(defs[&AgentPhase::Execute].max_iterations, u32::MAX);
        assert_eq!(defs[&AgentPhase::Execute].soft_limit_iterations, 0);
        assert_eq!(defs[&AgentPhase::Review].max_iterations, 5);
        assert_eq!(defs[&AgentPhase::Review].soft_limit_iterations, 3);
        assert_eq!(defs[&AgentPhase::Done].max_iterations, 0);
        assert_eq!(defs[&AgentPhase::Done].soft_limit_iterations, 0);
    }

    #[test]
    fn test_default_definitions_transitions() {
        let defs = default_definitions();
        // Research → Planning, Execute
        let r = &defs[&AgentPhase::Research];
        assert!(r.allowed_transitions.contains(&AgentPhase::Planning));
        assert!(r.allowed_transitions.contains(&AgentPhase::Execute));
        assert!(!r.allowed_transitions.contains(&AgentPhase::Review));
        assert!(!r.allowed_transitions.contains(&AgentPhase::Done));

        // Planning → Execute
        let p = &defs[&AgentPhase::Planning];
        assert!(p.allowed_transitions.contains(&AgentPhase::Execute));
        assert!(!p.allowed_transitions.contains(&AgentPhase::Review));

        // Execute → Review, Done
        let e = &defs[&AgentPhase::Execute];
        assert!(e.allowed_transitions.contains(&AgentPhase::Review));
        assert!(e.allowed_transitions.contains(&AgentPhase::Done));
        assert!(!e.allowed_transitions.contains(&AgentPhase::Planning));

        // Review → Planning, Execute, Done
        let rv = &defs[&AgentPhase::Review];
        assert!(rv.allowed_transitions.contains(&AgentPhase::Planning));
        assert!(rv.allowed_transitions.contains(&AgentPhase::Execute));
        assert!(rv.allowed_transitions.contains(&AgentPhase::Done));

        // Done → (none)
        assert!(defs[&AgentPhase::Done].allowed_transitions.is_empty());
    }

    #[test]
    fn test_files_written_gate_rejects_empty() {
        let gate = FilesWrittenGate;
        let ctx = PhaseContext::default();
        assert!(matches!(gate.check(&ctx), GateResult::Failed(_)));
    }

    #[test]
    fn test_files_written_gate_passes() {
        let gate = FilesWrittenGate;
        let ctx = PhaseContext {
            files_written: vec!["topics/plans/test.md".into()],
            ..Default::default()
        };
        assert!(matches!(gate.check(&ctx), GateResult::Passed));
    }

    #[test]
    fn test_entry_prompts_contain_phase_name() {
        let defs = default_definitions();
        let empty_ctx = ContextAccumulator::new();
        for (phase, name) in [
            (AgentPhase::Research, "Research"),
            (AgentPhase::Planning, "Planning"),
            (AgentPhase::Execute, "Execute"),
            (AgentPhase::Review, "Review"),
        ] {
            let prompt = (defs[&phase].entry_prompt)(&empty_ctx);
            assert!(prompt.contains(name), "{phase} entry prompt missing phase name");
        }
    }
}
