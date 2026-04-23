//! Session lifecycle plugins — decouple optional subsystems from AgentSession.

use std::sync::Arc;

use crate::session::WikiComponents;
use crate::token_tracker::ModelPricing;
use crate::wiki::{PageIndex, PageStore};

/// Plugin interface for mounting optional subsystems into a session.
///
/// Implementations include wiki knowledge injection, cost tracking,
/// and any other cross-cutting concern that should not bloat AgentSession.
pub(crate) trait SessionLifecyclePlugin: Send + Sync {
    /// Provide a memory loader for long-term knowledge injection.
    fn memory_loader(&self) -> Option<super::history::builder::MemoryLoader> {
        None
    }

    /// Provide wiki components if this plugin manages the wiki subsystem.
    fn wiki_components(&self) -> Option<&WikiComponents> {
        None
    }

    /// Provide pricing config if this plugin manages cost tracking.
    fn pricing(&self) -> Option<&ModelPricing> {
        None
    }

    /// Called after a response is finalized.
    fn on_response_finalized(
        &self,
        _response: &super::AgentResponse,
        _ctx: &super::FinalizeContext,
    ) {
    }
}

/// Wiki lifecycle plugin — owns wiki components and provides memory loader.
pub(crate) struct WikiLifecyclePlugin {
    components: WikiComponents,
}

impl WikiLifecyclePlugin {
    pub fn new(
        page_store: Arc<PageStore>,
        page_index: Arc<PageIndex>,
        wiki_log: Arc<crate::wiki::WikiLog>,
    ) -> Self {
        Self {
            components: WikiComponents {
                page_store,
                page_index,
                wiki_log,
            },
        }
    }
}

impl SessionLifecyclePlugin for WikiLifecyclePlugin {
    fn memory_loader(&self) -> Option<super::history::builder::MemoryLoader> {
        let page_index = self.components.page_index.clone();
        let page_store = self.components.page_store.clone();
        Some(Arc::new(move |user_input: &str| {
            let query = user_input.to_string();
            let index = page_index.clone();
            let store = page_store.clone();
            Box::pin(async move {
                let hits = index
                    .search_with_store(&query, 3, Some(&store))
                    .await
                    .ok()?;
                if hits.is_empty() {
                    return None;
                }
                let mut parts =
                    vec!["[Relevant long-term memories loaded for this turn]".to_string()];
                for hit in hits {
                    if let Ok(page) = store.read(&hit.path).await {
                        let preview = if page.content.chars().count() > 800 {
                            page.content.chars().take(800).collect::<String>() + "..."
                        } else {
                            page.content.clone()
                        };
                        parts.push(format!(
                            "## {} (path: {} | tags: [{}])\n{}",
                            page.title,
                            page.path,
                            page.tags.join(", "),
                            preview
                        ));
                    }
                }
                if parts.len() > 1 {
                    Some(parts.join("\n\n"))
                } else {
                    None
                }
            })
        }))
    }

    fn wiki_components(&self) -> Option<&WikiComponents> {
        Some(&self.components)
    }
}

/// Cost tracking plugin — calculates and logs token costs via pricing config.
pub(crate) struct CostTrackingPlugin {
    pricing: ModelPricing,
}

impl CostTrackingPlugin {
    pub fn new(pricing: ModelPricing) -> Self {
        Self { pricing }
    }
}

impl SessionLifecyclePlugin for CostTrackingPlugin {
    fn pricing(&self) -> Option<&ModelPricing> {
        Some(&self.pricing)
    }

    fn on_response_finalized(
        &self,
        response: &super::AgentResponse,
        _ctx: &super::FinalizeContext,
    ) {
        if let Some(ref usage) = response.token_usage {
            let cost = self
                .pricing
                .calculate_cost(usage.input_tokens, usage.output_tokens);
            tracing::info!(
                "[Token] Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
                usage.input_tokens,
                usage.output_tokens,
                usage.total_tokens,
                cost
            );
        }
    }
}
