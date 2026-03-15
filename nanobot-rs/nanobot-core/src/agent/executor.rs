//! Tool executor: handles tool call execution independently of the agent loop

use std::time::Instant;

use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use crate::providers::ToolCall;
use crate::tools::ToolRegistry;

/// Result of executing a single tool call
pub struct ToolCallResult {
    /// The original tool call ID
    pub tool_call_id: String,
    /// The tool name
    pub tool_name: String,
    /// The result string (success or formatted error)
    pub output: String,
}

/// Executes tool calls against a `ToolRegistry`, independent of the agent loop.
///
/// This struct owns no state beyond a reference to the registry and a
/// truncation limit, making it easy to test in isolation.
pub struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    max_result_chars: usize,
}

impl<'a> ToolExecutor<'a> {
    /// Create a new executor.
    ///
    /// `max_result_chars` of 0 means unlimited.
    pub fn new(registry: &'a ToolRegistry, max_result_chars: usize) -> Self {
        Self {
            registry,
            max_result_chars,
        }
    }

    /// Execute a single tool call and return the result.
    #[instrument(name = "executor.execute_one", skip_all, fields(tool = %tool_call.function.name))]
    pub async fn execute_one(&self, tool_call: &ToolCall) -> ToolCallResult {
        info!(
            "Tool call: {}({:?})",
            tool_call.function.name, tool_call.function.arguments
        );

        let start = Instant::now();
        let result = self
            .registry
            .execute(
                &tool_call.function.name,
                tool_call.function.arguments.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e));
        let elapsed = start.elapsed();

        match &result {
            Ok(output) => {
                debug!(
                    tool = %tool_call.function.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    output_len = output.len(),
                    "Tool completed"
                );
            }
            Err(e) => {
                warn!(
                    tool = %tool_call.function.name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Tool error"
                );
            }
        }

        let mut result_str = match result {
            Ok(r) => r,
            Err(e) => format!("Error: {}", e),
        };

        if self.max_result_chars > 0 && result_str.len() > self.max_result_chars {
            // O(1) UTF-8 boundary check: walk backwards to find a valid char boundary
            let mut end = self.max_result_chars;
            while !result_str.is_char_boundary(end) {
                end -= 1;
            }
            result_str.truncate(end);
            result_str.push_str("\n\n[... truncated]");
        }

        ToolCallResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.function.name.clone(),
            output: result_str,
        }
    }

    /// Execute a single tool call by name and raw arguments (convenience method).
    pub async fn execute_raw(&self, name: &str, args: Value) -> String {
        let start = Instant::now();
        let result = self
            .registry
            .execute(name, args)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e));
        let elapsed = start.elapsed();

        match &result {
            Ok(output) => {
                debug!(
                    tool = %name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    output_len = output.len(),
                    "Tool completed"
                );
            }
            Err(e) => {
                warn!(
                    tool = %name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Tool error"
                );
            }
        }

        let mut result_str = match result {
            Ok(r) => r,
            Err(e) => format!("Error: {}", e),
        };

        if self.max_result_chars > 0 && result_str.len() > self.max_result_chars {
            // O(1) UTF-8 boundary check: walk backwards to find a valid char boundary
            let mut end = self.max_result_chars;
            while !result_str.is_char_boundary(end) {
                end -= 1;
            }
            result_str.truncate(end);
            result_str.push_str("\n\n[... truncated]");
        }

        result_str
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolError, ToolRegistry, ToolResult as TResult};
    use async_trait::async_trait;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes back the input"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, args: Value) -> TResult {
            Ok(args.to_string())
        }
    }

    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _args: Value) -> TResult {
            Err(ToolError::ExecutionError("boom".to_string()))
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(FailTool));
        reg
    }

    #[tokio::test]
    async fn test_execute_one_success() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("call_1", "echo", serde_json::json!({"msg": "hi"}));
        let result = executor.execute_one(&tc).await;

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.tool_name, "echo");
        assert!(result.output.contains("hi"));
    }

    #[tokio::test]
    async fn test_execute_one_failure() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("call_2", "fail", serde_json::json!({}));
        let result = executor.execute_one(&tc).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 10);

        let tc = ToolCall::new(
            "c1",
            "echo",
            serde_json::json!({"long": "abcdefghijklmnopqrstuvwxyz"}),
        );
        let result = executor.execute_one(&tc).await;

        assert!(result.output.len() <= 10 + "\n\n[... truncated]".len());
        assert!(result.output.ends_with("[... truncated]"));
    }

    #[tokio::test]
    async fn test_not_found_tool() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0);

        let tc = ToolCall::new("c1", "nonexistent", serde_json::json!({}));
        let result = executor.execute_one(&tc).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation_multibyte_utf8() {
        // Test that truncation handles multi-byte UTF-8 characters correctly
        // without panicking on character boundaries
        let reg = make_registry();
        // 10 bytes would split a Chinese character (3 bytes each)
        let executor = ToolExecutor::new(&reg, 10);

        let tc = ToolCall::new(
            "c1",
            "echo",
            // Each Chinese character is 3 bytes in UTF-8
            serde_json::json!({"text": "你好世界测试数据更多内容"}),
        );
        let result = executor.execute_one(&tc).await;

        // Should not panic and should end with truncated marker
        assert!(result.output.ends_with("[... truncated]"));
        // The truncated content should be valid UTF-8
        assert!(result.output.is_char_boundary(result.output.len()));
    }
}
