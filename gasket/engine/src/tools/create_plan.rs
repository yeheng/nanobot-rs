//! Tool: create_plan — generate a Markdown execution plan for complex tasks.
//!
//! NO JSON AST — Markdown is the native data structure for LLM-to-LLM communication.
//! The LLM calls this when it determines a task requires multiple steps.
//! The plan is persisted to the wiki as a `PageType::Topic` page.

use std::sync::Arc;

use tracing::info;

use crate::wiki::{PageStore, PageType, WikiPage};
use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

/// Simplified create_plan tool — Markdown-based, no JSON AST.
///
/// Prompts the LLM for a structured Markdown plan, persists as WikiPage,
/// returns confirmation + file path to the caller.
pub struct CreatePlanTool {
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Arc<PageStore>,
}

impl CreatePlanTool {
    pub fn new(provider: Arc<dyn LlmProvider>, model: String, page_store: Arc<PageStore>) -> Self {
        Self {
            provider,
            model,
            page_store,
        }
    }

    pub async fn invoke(
        &self,
        goal: &str,
        context: &[ChatMessage],
    ) -> Result<(String, String), anyhow::Error> {
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
            return Err(anyhow::anyhow!("LLM returned empty plan"));
        }

        // Persist as WikiPage — no JSON AST, just Markdown
        let slug = slugify(goal);
        let path = format!("plans/{}", slug);

        let page = WikiPage::new(
            path.clone(),
            format!("Plan: {}", goal),
            PageType::Topic,
            plan_markdown,
        );

        self.page_store.write(&page).await?;
        info!("create_plan: persisted plan to {}", path);

        let confirmation = format!(
            "Plan created and saved to {}. The agent will now execute each step.",
            path
        );
        Ok((confirmation, path))
    }

    fn build_plan_prompt(&self, goal: &str, context: &[ChatMessage]) -> String {
        let context_text = context
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Goal: {}\n\n\
             Recent context:\n{}\n\n\
             Generate a structured execution plan in Markdown. Use:\n\
             - ## headers for phases\n\
             - - [ ] checklists for steps\n\
             - Mark step type inline: [D]irect, [P]arallel/delegated, [?]conditional\n\
             - Include a ## Verification section at the end\n\
             Do NOT output JSON.",
            goal, context_text
        )
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
