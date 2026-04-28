//! Shared formatting helpers for tool output.

use super::SubagentResult;

/// Extract a JSON array from an LLM response that may contain extraneous text.
///
/// Attempts four strategies in order:
/// 1. Direct parse of the trimmed input
/// 2. Extract from a markdown code block (```json ... ``` or ``` ... ```)
/// 3. Find the first `[` and last `]` and parse the slice
/// 4. Final direct parse (for a clear error message)
pub fn extract_json_array<T: serde::de::DeserializeOwned>(
    text: &str,
) -> Result<T, serde_json::Error> {
    let trimmed = text.trim();

    // 1. Direct parse first.
    if let Ok(val) = serde_json::from_str::<T>(trimmed) {
        return Ok(val);
    }

    // 2. Extract from markdown code blocks.
    static CODE_BLOCK_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let code_block_re = CODE_BLOCK_RE
        .get_or_init(|| regex::Regex::new(r"(?s)```(?:json)?\s*(\[.*?\])\s*```").unwrap());
    if let Some(caps) = code_block_re.captures(trimmed) {
        if let Some(block) = caps.get(1) {
            if let Ok(val) = serde_json::from_str::<T>(block.as_str()) {
                return Ok(val);
            }
        }
    }

    // 3. Fallback: find the first '[' and last ']'.
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            if end > start {
                if let Ok(val) = serde_json::from_str::<T>(&trimmed[start..=end]) {
                    return Ok(val);
                }
            }
        }
    }

    // 4. Final attempt for a clear error message.
    serde_json::from_str::<T>(trimmed)
}

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
