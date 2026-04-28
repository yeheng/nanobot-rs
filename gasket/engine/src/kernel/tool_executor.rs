//! Tool execution for the kernel.

use std::time::{Duration, Instant};

use tracing::{debug, info, instrument, warn};

use crate::tools::{ToolContext, ToolRegistry};
use gasket_providers::ToolCall;

/// Result of executing a single tool call
#[derive(Debug)]
pub struct ToolCallResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output: String,
}

/// Executes tool calls against a `ToolRegistry`.
pub struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    max_result_chars: usize,
    tool_timeout: Duration,
}

impl<'a> ToolExecutor<'a> {
    pub fn new(
        registry: &'a ToolRegistry,
        max_result_chars: usize,
        tool_timeout: Duration,
    ) -> Self {
        Self {
            registry,
            max_result_chars,
            tool_timeout,
        }
    }

    #[instrument(name = "kernel.execute_tool", skip_all, fields(tool = %tool_call.function.name))]
    pub async fn execute_one(&self, tool_call: &ToolCall, ctx: &ToolContext) -> ToolCallResult {
        info!(
            "Tool call: {}({:?})",
            tool_call.function.name, tool_call.function.arguments
        );

        let start = Instant::now();
        let result = tokio::time::timeout(
            self.tool_timeout,
            self.registry.execute(
                &tool_call.function.name,
                tool_call.function.arguments.clone(),
                ctx,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Tool execution timed out after {:?}", self.tool_timeout))
        .and_then(|r| r.map_err(|e| anyhow::anyhow!("{}", e)));
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
            let original_len = result_str.len();
            let end = result_str.floor_char_boundary(self.max_result_chars);
            result_str.truncate(end);
            result_str.push_str(&format!(
                "\n\n[OUTPUT TRUNCATED: original {} chars exceeded limit of {} chars]",
                original_len, self.max_result_chars
            ));
        }

        ToolCallResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.function.name.clone(),
            output: result_str,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolError, ToolResult as TResult};
    use async_trait::async_trait;
    use serde_json::Value;

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
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        async fn execute(&self, args: Value, _ctx: &ToolContext) -> TResult {
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
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> TResult {
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
        let executor = ToolExecutor::new(&reg, 0, std::time::Duration::from_secs(60));

        let tc = ToolCall::new("call_1", "echo", serde_json::json!({"msg": "hi"}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.tool_name, "echo");
        assert!(result.output.contains("hi"));
    }

    #[tokio::test]
    async fn test_execute_one_failure() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0, std::time::Duration::from_secs(60));

        let tc = ToolCall::new("call_2", "fail", serde_json::json!({}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 10, std::time::Duration::from_secs(60));

        let tc = ToolCall::new(
            "c1",
            "echo",
            serde_json::json!({"long": "abcdefghijklmnopqrstuvwxyz"}),
        );
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.contains("[OUTPUT TRUNCATED:"));
        assert!(result.output.contains("exceeded limit of 10 chars]"));
        let suffix_start = result.output.find("\n\n[OUTPUT TRUNCATED").unwrap();
        assert!(suffix_start <= 10);
    }

    #[tokio::test]
    async fn test_not_found_tool() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 0, std::time::Duration::from_secs(60));

        let tc = ToolCall::new("c1", "nonexistent", serde_json::json!({}));
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_truncation_multibyte_utf8() {
        let reg = make_registry();
        let executor = ToolExecutor::new(&reg, 10, std::time::Duration::from_secs(60));

        let tc = ToolCall::new(
            "c1",
            "echo",
            serde_json::json!({"text": "你好世界测试数据更多内容"}),
        );
        let result = executor.execute_one(&tc, &ToolContext::default()).await;

        assert!(result.output.contains("[OUTPUT TRUNCATED:"));
        assert!(result.output.is_char_boundary(result.output.len()));
    }
}
