//! Tool: create_plan — generate a Markdown execution plan for complex tasks.
//!
//! Uses the SubagentSpawner to delegate LLM calls, ensuring token tracking,
//! hooks, and streaming all work correctly.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use tracing::info;

use gasket_wiki::{slugify, PageStore, PageType, WikiPage};

use super::{simple_schema, Tool, ToolContext, ToolError, ToolOutput, ToolResult};

const DEFAULT_PLANNING_TASK: &str = "\
You are a planning assistant. \
Generate a structured execution plan in Markdown format. \
Use headers, checklists (- [ ]), and specify dependencies. \
Do NOT output JSON.\n\n";

pub struct CreatePlanTool {
    page_store: PageStore,
    planning_prompt: Option<String>,
}

impl CreatePlanTool {
    pub fn new(
        page_store: PageStore,
        planning_prompt: Option<String>,
    ) -> Self {
        Self {
            page_store,
            planning_prompt,
        }
    }

    async fn persist_plan(
        &self,
        goal: &str,
        plan_markdown: &str,
    ) -> Result<ToolOutput, anyhow::Error> {
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
        Ok(ToolOutput::text(format!(
            "{}\nPath: {}\n\n--- Plan ---\n{}",
            confirmation, path, plan_markdown
        )))
    }
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
            page_store: self.page_store.clone(),
            planning_prompt: self.planning_prompt.clone(),
        }))
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let parsed: CreatePlanArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let goal = parsed.goal.trim();
        if goal.is_empty() {
            return Err(ToolError::InvalidArguments(
                "goal must not be empty".to_string(),
            ));
        }

        let context_section = if parsed.context.is_empty() {
            "No additional context provided.".to_string()
        } else {
            parsed.context
        };

        let template = self
            .planning_prompt
            .as_deref()
            .unwrap_or(DEFAULT_PLANNING_TASK);

        let task = format!(
            "{}Goal: {}\n\nContext: {}",
            template, goal, context_section
        );

        let result = ctx
            .spawner
            .spawn(task, None)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Plan generation failed: {}", e)))?;

        let plan_markdown = result.response.content;

        if plan_markdown.is_empty() {
            return Err(ToolError::ExecutionError(
                "Subagent returned empty plan".to_string(),
            ));
        }

        self.persist_plan(goal, &plan_markdown)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Plan persistence failed: {}", e)))
    }
}
