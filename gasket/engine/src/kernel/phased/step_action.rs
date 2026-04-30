use crate::kernel::steppable_executor::StepResult;
use super::agent_phase::AgentPhase;

/// Classification of a `StepResult` produced by one LLM iteration.
#[derive(Debug, PartialEq)]
pub enum StepAction {
    /// Keep running tool calls in the current phase.
    Continue,
    /// LLM returned text without tool calls — pause for user input.
    WaitForUserInput,
    /// LLM invoked `phase_transition` — switch to the indicated phase.
    PhaseTransition { to: AgentPhase },
}

impl StepAction {
    /// Classify a `StepResult` into the next action the agent loop should take.
    pub fn classify(result: &StepResult) -> Self {
        // Phase transitions take priority — scan for a `phase_transition` tool call.
        for tc in &result.response.tool_calls {
            if tc.function.name == "phase_transition" {
                if let Some(phase_str) = tc.function.arguments.get("phase").and_then(|v| v.as_str()) {
                    if let Ok(to) = AgentPhase::try_from(phase_str) {
                        return StepAction::PhaseTransition { to };
                    }
                }
            }
        }
        // No tool calls + text content means the LLM wants user input.
        if !result.response.has_tool_calls() && result.response.content.is_some() {
            return StepAction::WaitForUserInput;
        }
        StepAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::steppable_executor::StepResult;
    use gasket_providers::{ChatResponse, ToolCall};

    fn make_step_result_with_tools(tools: Vec<(&str, &str)>) -> StepResult {
        let tool_calls: Vec<ToolCall> = tools
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| {
                ToolCall::new(
                    format!("call_{}", i),
                    name,
                    serde_json::from_str(args).unwrap_or_default(),
                )
            })
            .collect();
        StepResult {
            response: ChatResponse {
                content: None,
                tool_calls,
                reasoning_content: None,
                usage: None,
            },
            tool_results: vec![],
            should_continue: true,
        }
    }

    fn make_step_result_with_content(text: &str) -> StepResult {
        StepResult {
            response: ChatResponse {
                content: Some(text.to_string()),
                tool_calls: vec![],
                reasoning_content: None,
                usage: None,
            },
            tool_results: vec![],
            should_continue: false,
        }
    }

    #[test]
    fn test_classify_phase_transition() {
        let result = make_step_result_with_tools(vec![
            ("phase_transition", r#"{"phase":"execute"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to } if to == AgentPhase::Execute));
    }

    #[test]
    fn test_classify_text_response() {
        let result = make_step_result_with_content("Can you clarify?");
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::WaitForUserInput));
    }

    #[test]
    fn test_classify_other_tool_calls_continue() {
        let result = make_step_result_with_tools(vec![
            ("wiki_search", r#"{"query":"test"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::Continue));
    }

    #[test]
    fn test_classify_mixed_tools_prioritizes_phase_transition() {
        let result = make_step_result_with_tools(vec![
            ("wiki_search", r#"{"query":"test"}"#),
            ("phase_transition", r#"{"phase":"planning"}"#),
        ]);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to } if to == AgentPhase::Planning));
    }
}
