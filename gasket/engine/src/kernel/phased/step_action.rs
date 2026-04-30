use crate::kernel::steppable_executor::StepResult;
use crate::tools::ToolControlSignal;
use super::agent_phase::AgentPhase;
use gasket_providers::FinishReason;

/// Classification of a `StepResult` produced by one LLM iteration.
#[derive(Debug, PartialEq)]
pub enum StepAction {
    /// Keep running tool calls in the current phase.
    Continue,
    /// LLM returned text without tool calls — pause for user input.
    WaitForUserInput,
    /// A tool emitted a `TransitionPhase` signal — switch to the indicated phase.
    PhaseTransition { to: AgentPhase, context_summary: Option<String> },
}

impl StepAction {
    /// Classify a `StepResult` into the next action the agent loop should take.
    ///
    /// Inspects tool-execution signals (not tool names) for phase transitions,
    /// then falls back to content-based heuristics for `WaitForUserInput`.
    /// Only triggers `WaitForUserInput` when `finish_reason` is `Stop`
    /// (natural end) — NOT `Length` (truncation).
    pub fn classify(result: &StepResult) -> Self {
        // Phase transitions take priority — look for a control signal.
        if let Some(ToolControlSignal::TransitionPhase { phase, context_summary }) =
            result.control_signal()
        {
            if let Ok(to) = AgentPhase::try_from(phase.as_str()) {
                return StepAction::PhaseTransition {
                    to,
                    context_summary: context_summary.clone(),
                };
            }
        }
        // WaitForUserInput only when:
        // 1. No tool calls
        // 2. Has text content
        // 3. Finish reason is Stop (natural end), not Length (truncation)
        if !result.response.has_tool_calls() && result.response.content.is_some() {
            let is_natural_stop = result
                .response
                .finish_reason
                .as_ref()
                .map_or(true, |r| matches!(r, FinishReason::Stop));
            if is_natural_stop {
                return StepAction::WaitForUserInput;
            }
        }
        StepAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::steppable_executor::StepResult;
    use crate::kernel::tool_executor::ToolCallResult;
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
                finish_reason: None,
            },
            tool_results: vec![],
            should_continue: true,
        }
    }

    fn make_step_result_with_signal(phase: &str, summary: Option<&str>) -> StepResult {
        StepResult {
            response: ChatResponse {
                content: None,
                tool_calls: vec![ToolCall::new(
                    "call_0",
                    "phase_transition",
                    serde_json::json!({"phase": phase}),
                )],
                reasoning_content: None,
                usage: None,
                finish_reason: None,
            },
            tool_results: vec![ToolCallResult {
                tool_call_id: "call_0".into(),
                tool_name: "phase_transition".into(),
                output: format!("Phase transition to {} acknowledged.", phase),
                signal: Some(ToolControlSignal::TransitionPhase {
                    phase: phase.to_string(),
                    context_summary: summary.map(|s| s.to_string()),
                }),
            }],
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
                finish_reason: Some(FinishReason::Stop),
            },
            tool_results: vec![],
            should_continue: false,
        }
    }

    #[test]
    fn test_classify_phase_transition() {
        let result = make_step_result_with_signal("execute", None);
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to, .. } if to == AgentPhase::Execute));
    }

    #[test]
    fn test_classify_phase_transition_with_summary() {
        let result = make_step_result_with_signal("planning", Some("Found wiki pages"));
        let action = StepAction::classify(&result);
        assert!(matches!(action, StepAction::PhaseTransition { to, context_summary: Some(s), .. } if to == AgentPhase::Planning && s == "Found wiki pages"));
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
    fn test_no_hardcoded_tool_name() {
        // A step result with phase_transition tool call but NO signal
        // should NOT trigger PhaseTransition (kernel doesn't look at tool names).
        let result = make_step_result_with_tools(vec![
            ("phase_transition", r#"{"phase":"execute"}"#),
        ]);
        // No signal in tool_results — should be Continue, not PhaseTransition
        assert!(matches!(StepAction::classify(&result), StepAction::Continue));
    }

    #[test]
    fn test_length_truncation_does_not_wait_for_user() {
        // When finish_reason is Length (truncation), should Continue, not WaitForUserInput
        let result = StepResult {
            response: ChatResponse {
                content: Some("I was in the middle of...".to_string()),
                tool_calls: vec![],
                reasoning_content: None,
                usage: None,
                finish_reason: Some(FinishReason::Length),
            },
            tool_results: vec![],
            should_continue: false,
        };
        assert!(matches!(StepAction::classify(&result), StepAction::Continue));
    }
}
