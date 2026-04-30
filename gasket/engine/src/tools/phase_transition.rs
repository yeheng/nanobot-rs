//! Phase transition tool — signal transition between working phases.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolContext, ToolControlSignal, ToolError, ToolOutput, ToolResult};

/// Tool for transitioning between agent working phases.
pub struct PhaseTransitionTool;

impl Default for PhaseTransitionTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseTransitionTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TransitionArgs {
    phase: String,
    #[serde(default)]
    #[allow(dead_code)]
    context_summary: String,
}

#[async_trait]
impl Tool for PhaseTransitionTool {
    fn name(&self) -> &str {
        "phase_transition"
    }

    fn description(&self) -> &str {
        "Transition to the next working phase. Valid targets depend on current phase: \
         Research -> planning|execute, Planning -> execute, Execute -> review|done, \
         Review -> done. Optionally provide a context_summary for the next phase."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "phase": {
                    "type": "string",
                    "enum": ["planning", "execute", "review", "done"],
                    "description": "Target phase"
                },
                "context_summary": {
                    "type": "string",
                    "description": "Optional summary for the next phase"
                }
            },
            "required": ["phase"]
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: TransitionArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let valid = ["planning", "execute", "review", "done"];
        if !valid.contains(&parsed.phase.as_str()) {
            return Err(ToolError::InvalidArguments(format!(
                "Invalid phase '{}'. Valid: {:?}",
                parsed.phase, valid
            )));
        }

        let summary = if parsed.context_summary.is_empty() {
            None
        } else {
            Some(parsed.context_summary.clone())
        };

        Ok(ToolOutput::with_signal(
            format!("Phase transition to {} acknowledged.", parsed.phase),
            ToolControlSignal::TransitionPhase {
                phase: parsed.phase.clone(),
                context_summary: summary,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = PhaseTransitionTool::new();
        assert_eq!(tool.name(), "phase_transition");
        assert!(tool.description().contains("phase"));
    }

    #[tokio::test]
    async fn test_execute_valid_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({"phase": "execute", "context_summary": "Found wiki pages"});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.content.contains("execute"));
        assert!(output.signal.is_some());
    }

    #[tokio::test]
    async fn test_execute_missing_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_invalid_phase() {
        let tool = PhaseTransitionTool::new();
        let args = serde_json::json!({"phase": "invalid_phase"});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }
}
