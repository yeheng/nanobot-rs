//! Tool: create_plan — generate a Markdown execution plan for complex tasks.
//!
//! NO JSON AST — Markdown is the native data structure for LLM-to-LLM communication.
//! The LLM calls this when it determines a task requires multiple steps.
//! The plan is persisted to the wiki as a `PageType::Topic` page.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use tracing::info;

use gasket_wiki::{PageStore, PageType, WikiPage};
use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};

/// Default planning user prompt template (fallback when not configured).
/// Must contain `{{goal}}` and `{{context}}` which will be replaced at runtime.
const DEFAULT_PLANNING_TEMPLATE: &str = "Goal: {{goal}}\n\n\
     Recent context:\n{{context}}\n\n\
     Generate a structured execution plan in Markdown. Use:\n\
     - ## headers for phases\n\
     - - [ ] checklists for steps\n\
     - Mark step type inline: [D]irect, [P]arallel/delegated, [?]conditional\n\
     - Include a ## Verification section at the end\n\
     Do NOT output JSON.";

/// Simplified create_plan tool — Markdown-based, no JSON AST.
///
/// Prompts the LLM for a structured Markdown plan, persists as WikiPage,
/// returns confirmation + file path to the caller.
pub struct CreatePlanTool {
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: PageStore,
    planning_prompt: Option<String>,
}

impl CreatePlanTool {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        page_store: PageStore,
        planning_prompt: Option<String>,
    ) -> Self {
        Self {
            provider,
            model,
            page_store,
            planning_prompt,
        }
    }

    pub async fn invoke(
        &self,
        goal: &str,
        context: &[ChatMessage],
    ) -> Result<(String, String, String), anyhow::Error> {
        let prompt = self.build_plan_prompt(goal, context);

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(
                    "You are a planning assistant. \
                     Generate a structured execution plan in Markdown format. \
                     Use headers, checklists (- [ ]), and specify dependencies. \
                     Do NOT output JSON.",
                ),
                ChatMessage::user(prompt),
            ],
            tools: None,
            temperature: Some(0.3),
            max_tokens: Some(2048),
            thinking: None,
        };

        let response = self.provider.chat(request).await?;
        let plan_markdown = response.content.unwrap_or_default();

        if plan_markdown.is_empty() {
            // Some reasoning models return thinking content but leave content empty.
            // Use reasoning_content as a fallback so planning doesn't hard-fail.
            if let Some(ref reasoning) = response.reasoning_content {
                if !reasoning.is_empty() {
                    tracing::warn!(
                        "create_plan: LLM returned empty content, falling back to reasoning_content ({} chars)",
                        reasoning.chars().count()
                    );
                    return self.persist_plan(goal, reasoning).await;
                }
            }
            return Err(anyhow::anyhow!("LLM returned empty plan"));
        }

        self.persist_plan(goal, &plan_markdown).await
    }

    async fn persist_plan(
        &self,
        goal: &str,
        plan_markdown: &str,
    ) -> Result<(String, String, String), anyhow::Error> {
        let slug = slugify(goal);
        let path = format!("topics/plans/{}", slug);

        let page = WikiPage::new(
            path.clone(),
            format!("Plan: {}", goal),
            PageType::Topic,
            plan_markdown.to_string(),
        );

        self.page_store.write(&page).await?;
        info!("create_plan: persisted plan to {}", path);

        let confirmation = format!(
            "Plan created and saved to {}. The agent will now execute each step.",
            path
        );
        Ok((confirmation, path, plan_markdown.to_string()))
    }

    fn build_plan_prompt(&self, goal: &str, context: &[ChatMessage]) -> String {
        let context_text = context
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");

        let template = self
            .planning_prompt
            .as_deref()
            .unwrap_or(DEFAULT_PLANNING_TEMPLATE);
        template
            .replace("{{goal}}", goal)
            .replace("{{context}}", &context_text)
    }
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .replace(" ", "-")
        .replace("/", "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

#[derive(Deserialize)]
struct CreatePlanArgs {
    goal: String,
    #[serde(default)]
    context: String,
}

#[async_trait]
impl Tool for CreatePlanTool {
    fn name(&self) -> &str {
        "create_plan"
    }

    fn description(&self) -> &str {
        "Generate a structured Markdown execution plan for a complex task and persist it to the wiki. \
         The LLM should call this when a user request clearly requires multiple steps or phases. \
         Returns a confirmation message and the wiki path where the plan was saved."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "goal",
                "string",
                true,
                "High-level goal or task description to plan for",
            ),
            (
                "context",
                "string",
                false,
                "Optional additional context to inform the plan",
            ),
        ])
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Option<Box<dyn Tool>> {
        Some(Box::new(Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            page_store: self.page_store.clone(),
            planning_prompt: self.planning_prompt.clone(),
        }))
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: CreatePlanArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let goal = parsed.goal.trim();
        if goal.is_empty() {
            return Err(ToolError::InvalidArguments(
                "goal must not be empty".to_string(),
            ));
        }

        let context_msg = if parsed.context.is_empty() {
            ChatMessage::system("No additional context provided.".to_string())
        } else {
            ChatMessage::user(parsed.context)
        };

        let (confirmation, path, plan) = self
            .invoke(goal, &[context_msg])
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Plan generation failed: {}", e)))?;

        Ok(format!(
            "{}\nPath: {}\n\n--- Plan ---\n{}",
            confirmation, path, plan
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Rust Setup"), "rust-setup");
        assert_eq!(slugify("CI/CD Pipeline"), "ci-cd-pipeline");
        assert_eq!(slugify("Hello World!"), "hello-world");
    }
}
