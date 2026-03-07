//! Delegate tool — inter-agent communication via the permission matrix.
//!
//! Allows an agent to invoke another agent within the pipeline hierarchy.
//! The permission matrix is checked before dispatch, and the target agent's
//! SOUL.md is injected as the system prompt.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::instrument;

use super::base::{Tool, ToolError, ToolResult};
use crate::agent::subagent::SubagentManager;
use crate::pipeline::permission::PermissionMatrix;

pub struct DelegateTool {
    subagent: Arc<SubagentManager>,
    permission_matrix: PermissionMatrix,
    /// Map from role name → SOUL.md content (loaded at init time).
    soul_templates: std::collections::HashMap<String, String>,
}

impl DelegateTool {
    pub fn new(
        subagent: Arc<SubagentManager>,
        permission_matrix: PermissionMatrix,
        soul_templates: std::collections::HashMap<String, String>,
    ) -> Self {
        Self {
            subagent,
            permission_matrix,
            soul_templates,
        }
    }
}

#[derive(Deserialize)]
struct DelegateArgs {
    /// The role name of the caller (self-declared; the orchestrator verifies).
    caller_role: String,
    /// The target role to delegate to.
    target_role: String,
    /// The task description / prompt to send.
    task_description: String,
    /// Whether to wait for the result (default: true).
    #[serde(default = "default_sync")]
    sync: bool,
}

fn default_sync() -> bool {
    true
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to another agent role in the pipeline hierarchy. \
         The permission matrix is enforced: only allowed caller→target pairs succeed."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "caller_role": {
                    "type": "string",
                    "description": "Your role name (e.g. zhongshu, shangshu)"
                },
                "target_role": {
                    "type": "string",
                    "description": "Target role to delegate to"
                },
                "task_description": {
                    "type": "string",
                    "description": "The task prompt to send to the target agent"
                },
                "sync": {
                    "type": "boolean",
                    "description": "Wait for result (true) or fire-and-forget (false). Default: true",
                    "default": true
                }
            },
            "required": ["caller_role", "target_role", "task_description"]
        })
    }

    #[instrument(name = "tool.delegate", skip_all)]
    async fn execute(&self, args: Value) -> ToolResult {
        let args: DelegateArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Permission check
        if !self
            .permission_matrix
            .is_allowed(&args.caller_role, &args.target_role)
        {
            return Err(ToolError::PermissionDenied(format!(
                "Role '{}' is not allowed to delegate to '{}'",
                args.caller_role, args.target_role
            )));
        }

        // Load target's SOUL.md as system prompt
        let system_prompt = self.soul_templates.get(&args.target_role);

        if args.sync {
            // Synchronous: wait for the agent's response
            let response = self
                .subagent
                .submit_and_wait(
                    &args.task_description,
                    system_prompt.map(|s| s.as_str()),
                )
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            Ok(serde_json::to_string_pretty(&serde_json::json!({
                "status": "completed",
                "target_role": args.target_role,
                "response": response.content,
            }))
            .unwrap())
        } else {
            // Async: fire-and-forget via regular submit
            self.subagent
                .submit(&args.task_description, "cli", "pipeline_async")
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            Ok(serde_json::to_string_pretty(&serde_json::json!({
                "status": "dispatched",
                "target_role": args.target_role,
            }))
            .unwrap())
        }
    }
}
