//! Shared formatting helpers for tool output.

use super::SubagentResult;

/// Format a single [`SubagentResult`] into a human-readable string.
pub fn format_subagent_response(result: &SubagentResult) -> String {
    let mut output = String::new();

    if let Some(ref reasoning) = result.response.reasoning_content {
        if !reasoning.is_empty() {
            output.push_str(&format!("**Thinking:**\n{}\n\n", reasoning));
        }
    }

    output.push_str(&format!(
        "**Model:** {}\n**Task:** {}\n\n**Response:**\n{}",
        result.model.as_deref().unwrap_or("unknown"),
        result.task,
        result.response.content
    ));

    output
}
