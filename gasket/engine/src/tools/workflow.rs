//! Native workflow engine — state-machine driven multi-step agent pipelines.
//!
//! Workflows are YAML-defined directed graphs (with loops for retries) that
//! orchestrate subagent spawning. Each step feeds its output into the next
//! step via simple `{{variable}}` template substitution.
//!
//! Key design decisions:
//! - Stateless: all execution context lives in a `HashMap<String, String>`.
//! - Crash-safe: no external daemon, no IPC, no Python interpreter.
//! - Streaming: each subagent step uses `spawn_with_stream` so the frontend
//!   sees real-time thinking and tool-call events.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};

use super::{Tool, ToolContext, ToolError, ToolResult};

// ── Data structures ─────────────────────────────────────────────────────────

/// A workflow manifest loaded from a YAML file.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowManifest {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub start_step: String,
    pub steps: HashMap<String, WorkflowStep>,
}

/// A single step in the workflow graph.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStep {
    /// Prompt template with `{{key}}` placeholders.
    pub prompt: String,
    /// Optional model override for this step.
    pub model: Option<String>,
    /// Next step name. Absent when `evaluate` is present.
    pub next: Option<String>,
    /// Evaluation configuration for verdict-based branching.
    pub evaluate: Option<EvaluateConfig>,
}

/// Configuration for evaluating a step's output and deciding the next step.
#[derive(Debug, Clone, Deserialize)]
pub struct EvaluateConfig {
    /// Step to go to when evaluation passes.
    pub on_pass: String,
    /// Step to go to when evaluation fails.
    pub on_fail: String,
    /// Maximum retry attempts before giving up.
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
}

fn default_max_retries() -> usize {
    3
}

// ── Template substitution ───────────────────────────────────────────────────

/// Substitute `{{key}}` placeholders in a template using values from `ctx`.
///
/// Unknown keys are left as-is so that missing variables are obvious in the
/// LLM prompt rather than silently replaced with empty strings.
fn substitute_template(template: &str, ctx: &HashMap<String, String>) -> String {
    // Regex for {{key}} where key is alphanumeric + dots + underscores + slashes
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\{\{([a-zA-Z0-9_./]+)\}\}").unwrap());
    let mut result = template.to_string();
    for caps in re.captures_iter(template) {
        let full = caps.get(0).unwrap().as_str();
        let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(val) = ctx.get(key) {
            result = result.replace(full, val);
        }
    }
    result
}

// ── Verdict parsing ─────────────────────────────────────────────────────────

/// Parse a verdict from LLM output.
///
/// Tolerates ```json fences and missing fields. Unparseable input is treated
/// as FAIL so the workflow retries rather than silently proceeding.
fn parse_verdict(text: &str) -> Result<(String, String), String> {
    let mut txt = text.trim();

    // Strip ```json ... ``` fences if present
    if txt.starts_with("```") {
        txt = txt.trim_start_matches('`');
        if txt.to_lowercase().starts_with("json") {
            txt = &txt[4..];
        }
        txt = txt.trim().trim_end_matches('`').trim();
    }

    let obj: Value = serde_json::from_str(txt).map_err(|e| {
        format!(
            "JSON parse error: {} in text: {}",
            e,
            &text[..text.len().min(200)]
        )
    })?;

    let verdict = obj
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("FAIL")
        .to_uppercase();

    let reason = obj
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let verdict = if verdict == "PASS" || verdict == "FAIL" {
        verdict
    } else {
        "FAIL".to_string()
    };

    Ok((verdict, reason))
}

// ── WorkflowTool ────────────────────────────────────────────────────────────

/// A tool that executes a YAML-defined workflow as a state machine.
#[derive(Clone)]
pub struct WorkflowTool {
    manifest: WorkflowManifest,
}

impl WorkflowTool {
    /// Create a new workflow tool from a manifest.
    pub fn new(manifest: WorkflowManifest) -> Self {
        Self { manifest }
    }

    /// Get the underlying manifest.
    pub fn manifest(&self) -> &WorkflowManifest {
        &self.manifest
    }

    /// Send a lightweight progress message to the user via WebSocket.
    async fn notify(ctx: &ToolContext, message: &str) {
        use gasket_types::events::{ChatEvent, OutboundMessage};
        let msg = OutboundMessage::with_ws_message(
            ctx.session_key.channel.clone(),
            ctx.session_key.chat_id.clone(),
            ChatEvent::text(message),
        );
        let _ = ctx.outbound_tx.send(msg).await;
    }

    /// Execute a single step: spawn subagent, stream events, collect result.
    async fn run_step(
        &self,
        step_name: &str,
        prompt: &str,
        model: Option<String>,
        ctx: &ToolContext,
    ) -> Result<String, ToolError> {
        let spawner = ctx.spawner.as_ref().ok_or_else(|| {
            ToolError::ExecutionError(
                "Subagent spawning is not available in this context".to_string(),
            )
        })?;

        info!(
            "[Workflow {}] Step '{}' spawning subagent",
            self.manifest.name, step_name
        );

        // Spawn with streaming so the frontend sees real-time progress.
        let (subagent_id, mut event_rx, result_rx, _cancel_token) = spawner
            .spawn_with_stream(prompt.to_string(), model)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e)))?;

        // Forward streaming events to WebSocket in the background.
        let fwd_session_key = ctx.session_key.clone();
        let fwd_outbound_tx = ctx.outbound_tx.clone();
        let fwd_subagent_id = subagent_id.clone();
        let _forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                use gasket_types::events::{ChatEvent, OutboundMessage};
                use gasket_types::StreamEventKind;

                let chat_event = match &event.kind {
                    StreamEventKind::Thinking { content } => Some(ChatEvent::subagent_thinking(
                        &fwd_subagent_id,
                        content.as_ref(),
                    )),
                    StreamEventKind::ToolStart { name, arguments } => {
                        Some(ChatEvent::subagent_tool_start(
                            &fwd_subagent_id,
                            name.as_ref(),
                            arguments.as_ref().map(|s| s.to_string()),
                        ))
                    }
                    StreamEventKind::ToolEnd { name, output } => {
                        Some(ChatEvent::subagent_tool_end(
                            &fwd_subagent_id,
                            name.as_ref(),
                            output.as_ref().map(|s| s.to_string()),
                        ))
                    }
                    StreamEventKind::Content { content } => Some(ChatEvent::subagent_content(
                        &fwd_subagent_id,
                        content.as_ref(),
                    )),
                    _ => None,
                };

                if let Some(chat_event) = chat_event {
                    let msg = OutboundMessage::with_ws_message(
                        fwd_session_key.channel.clone(),
                        fwd_session_key.chat_id.clone(),
                        chat_event,
                    );
                    let _ = fwd_outbound_tx.send(msg).await;
                }
            }
        });

        // Block for the final result.
        let result = result_rx.await.map_err(|e| {
            ToolError::ExecutionError(format!("Subagent result channel closed: {}", e))
        })?;

        info!(
            "[Workflow {}] Step '{}' completed (tools_used: {})",
            self.manifest.name,
            step_name,
            result.response.tools_used.len()
        );

        Ok(result.response.content)
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn description(&self) -> &str {
        &self.manifest.description
    }

    fn parameters(&self) -> Value {
        self.manifest.parameters.clone()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Option<Box<dyn Tool>> {
        Some(Box::new(self.clone()))
    }

    #[tracing::instrument(name = "tool.workflow", skip_all, fields(workflow = %self.manifest.name))]
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        // Flatten user arguments into the context map under the "input." prefix.
        let mut context_map: HashMap<String, String> = HashMap::new();
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                context_map.insert(
                    format!("input.{}", k),
                    v.to_string().trim_matches('"').to_string(),
                );
            }
        }

        let mut current_step = self.manifest.start_step.clone();
        let mut retry_counts: HashMap<String, usize> = HashMap::new();

        while current_step != "DONE" {
            let step = self.manifest.steps.get(&current_step).ok_or_else(|| {
                ToolError::ExecutionError(format!("Workflow step '{}' not found", current_step))
            })?;

            // Substitute template variables.
            let prompt = substitute_template(&step.prompt, &context_map);

            // Notify user of progress.
            Self::notify(
                ctx,
                &format!("🔄 **{}**: {}", current_step, &self.manifest.name),
            )
            .await;

            // Execute the step via subagent.
            let result = self
                .run_step(&current_step, &prompt, step.model.clone(), ctx)
                .await?;

            // Store result keyed by step name.
            context_map.insert(current_step.clone(), result);

            // Determine next step.
            if let Some(ref eval) = step.evaluate {
                let review_text = context_map.get(&current_step).cloned().unwrap_or_default();
                let (verdict, reason) = parse_verdict(&review_text).unwrap_or_else(|e| {
                    warn!(
                        "[Workflow {}] Verdict parse failed for step '{}': {}",
                        self.manifest.name, current_step, e
                    );
                    ("FAIL".to_string(), e)
                });

                // Store reason for template use in the loop-back step.
                context_map.insert(format!("{}_reason", current_step), reason.clone());

                if verdict == "PASS" {
                    Self::notify(ctx, &format!("✅ **{}** passed", current_step)).await;
                    current_step = eval.on_pass.clone();
                } else {
                    let retries = retry_counts.entry(current_step.clone()).or_insert(0);
                    *retries += 1;
                    if *retries > eval.max_retries {
                        return Err(ToolError::ExecutionError(format!(
                            "Workflow step '{}' failed after {} retries. Last reason: {}",
                            current_step, eval.max_retries, reason
                        )));
                    }
                    Self::notify(
                        ctx,
                        &format!(
                            "❌ **{}** failed (retry {}/{}): {}",
                            current_step, retries, eval.max_retries, reason
                        ),
                    )
                    .await;
                    current_step = eval.on_fail.clone();
                }
            } else if let Some(ref next) = step.next {
                current_step = next.clone();
            } else {
                return Err(ToolError::ExecutionError(format!(
                    "Workflow step '{}' has no 'next' and no 'evaluate'",
                    current_step
                )));
            }
        }

        // Build final result JSON.
        let final_output = serde_json::json!({
            "context": context_map,
        });
        serde_json::to_string(&final_output)
            .map_err(|e| ToolError::ExecutionError(format!("Failed to serialize result: {}", e)))
    }
}

// ── Discovery ─────────────────────────────────────────────────────────────────

/// Discover and load all workflow manifests in a directory.
///
/// Scans `*.yaml` and `*.yml` files, parses them as `WorkflowManifest`,
/// and wraps each in a `WorkflowTool`.
pub fn discover_workflows(workflows_dir: &Path) -> anyhow::Result<Vec<WorkflowTool>> {
    let mut tools = Vec::new();

    if !workflows_dir.exists() {
        tracing::info!(
            "Workflows directory does not exist: {:?}, skipping discovery",
            workflows_dir
        );
        return Ok(tools);
    }

    let entries = std::fs::read_dir(workflows_dir).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read workflows directory {:?}: {}",
            workflows_dir,
            e
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!(
                "Failed to read directory entry in {:?}: {}",
                workflows_dir,
                e
            )
        })?;

        let path = entry.path();
        if path.is_dir() {
            continue;
        }

        let ext = path.extension().and_then(|s| s.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        match load_workflow(&path) {
            Ok(manifest) => {
                info!("Discovered workflow '{}' from {:?}", manifest.name, path);
                tools.push(WorkflowTool::new(manifest));
            }
            Err(e) => {
                warn!("Failed to load workflow from {:?}: {}", path, e);
            }
        }
    }

    Ok(tools)
}

/// Load a single workflow manifest from a YAML file.
fn load_workflow(path: &Path) -> anyhow::Result<WorkflowManifest> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read workflow file {:?}: {}", path, e))?;

    let manifest: WorkflowManifest = serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse workflow YAML from {:?}: {}", path, e))?;

    if manifest.name.is_empty() {
        return Err(anyhow::anyhow!("Workflow from {:?} has empty name", path));
    }
    if manifest.description.is_empty() {
        return Err(anyhow::anyhow!(
            "Workflow from {:?} has empty description",
            path
        ));
    }
    if manifest.steps.is_empty() {
        return Err(anyhow::anyhow!("Workflow from {:?} has no steps", path));
    }
    if !manifest.steps.contains_key(&manifest.start_step) {
        return Err(anyhow::anyhow!(
            "Workflow from {:?} start_step '{}' not found in steps",
            path,
            manifest.start_step
        ));
    }

    // Validate each step has either `next` or `evaluate`.
    for (name, step) in &manifest.steps {
        if step.next.is_none() && step.evaluate.is_none() {
            return Err(anyhow::anyhow!(
                "Workflow step '{}' in {:?} has neither 'next' nor 'evaluate'",
                name,
                path
            ));
        }
    }

    Ok(manifest)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_substitution_basic() {
        let mut ctx = HashMap::new();
        ctx.insert("input.task".to_string(), "build a cat".to_string());
        ctx.insert("research".to_string(), "cats are furry".to_string());

        let template = "Task: {{input.task}}\nResearch: {{research}}\nMissing: {{missing}}";
        let result = substitute_template(template, &ctx);
        assert_eq!(
            result,
            "Task: build a cat\nResearch: cats are furry\nMissing: {{missing}}"
        );
    }

    #[test]
    fn parse_verdict_clean_json() {
        let text = r#"{"verdict": "PASS", "reason": "looks good"}"#;
        let (v, r) = parse_verdict(text).unwrap();
        assert_eq!(v, "PASS");
        assert_eq!(r, "looks good");
    }

    #[test]
    fn parse_verdict_with_fences() {
        let text = "```json\n{\"verdict\": \"FAIL\", \"reason\": \"bad code\"}\n```";
        let (v, r) = parse_verdict(text).unwrap();
        assert_eq!(v, "FAIL");
        assert_eq!(r, "bad code");
    }

    #[test]
    fn parse_verdict_unparseable_defaults_to_fail() {
        let text = "this is not json at all";
        let result = parse_verdict(text);
        assert!(result.is_err());
    }

    #[test]
    fn parse_verdict_missing_verdict_defaults_to_fail() {
        let text = r#"{"reason": "no verdict field"}"#;
        let (v, r) = parse_verdict(text).unwrap();
        assert_eq!(v, "FAIL");
        assert_eq!(r, "no verdict field");
    }

    #[test]
    fn load_workflow_from_yaml() {
        let yaml = r#"
name: "test_workflow"
description: "A test workflow"
parameters:
  type: object
  properties:
    task:
      type: string
  required: ["task"]
start_step: "step1"
steps:
  step1:
    prompt: "Do {{input.task}}"
    next: "step2"
  step2:
    prompt: "Check {{step1}}"
    evaluate:
      on_pass: "DONE"
      on_fail: "step1"
      max_retries: 2
"#;
        let manifest: WorkflowManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "test_workflow");
        assert_eq!(manifest.start_step, "step1");
        assert_eq!(manifest.steps.len(), 2);
        assert_eq!(
            manifest.steps["step2"]
                .evaluate
                .as_ref()
                .unwrap()
                .max_retries,
            2
        );
    }

    #[test]
    fn workflow_tool_name_and_description() {
        let manifest = WorkflowManifest {
            name: "my_flow".to_string(),
            description: "does things".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            start_step: "a".to_string(),
            steps: {
                let mut m = HashMap::new();
                m.insert(
                    "a".to_string(),
                    WorkflowStep {
                        prompt: "hello".to_string(),
                        model: None,
                        next: Some("DONE".to_string()),
                        evaluate: None,
                    },
                );
                m
            },
        };
        let tool = WorkflowTool::new(manifest);
        assert_eq!(tool.name(), "my_flow");
        assert_eq!(tool.description(), "does things");
    }
}
