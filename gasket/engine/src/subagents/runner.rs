//! Subagent runner - extracted pure function + trait for model resolution

use std::sync::Arc;

use crate::kernel::{ExecutionResult, KernelExecutor};
use crate::session::config::{AgentConfig, AgentConfigExt};
use crate::tools::ToolRegistry;
use anyhow::Result;
use gasket_providers::{ChatMessage, LlmProvider};
use tracing::{info, warn};

/// Trait for resolving model IDs to providers and configs.
///
/// Implemented by the CLI layer using `ProviderRegistry` + `ModelRegistry`.
/// This decouples the engine from configuration details.
pub trait ModelResolver: Send + Sync {
    /// Resolve a model ID to a provider and agent config.
    ///
    /// Returns `None` if the model ID is not recognized.
    fn resolve_model(&self, model_id: &str) -> Option<(Arc<dyn LlmProvider>, AgentConfig)>;
}

/// Run a subagent with minimal overhead - pure function
pub async fn run_subagent(
    task: &str,
    system_prompt: &str,
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: &AgentConfig,
) -> Result<ExecutionResult, anyhow::Error> {
    info!("Running subagent with model={}", config.model);
    let messages = vec![ChatMessage::system(system_prompt), ChatMessage::user(task)];
    let kernel_config = config.to_kernel_config();
    let executor = KernelExecutor::new(provider, tools, &kernel_config);
    executor
        .execute_with_options(messages, &crate::kernel::ExecutorOptions::new())
        .await
        .map_err(|e| {
            warn!("Subagent execution failed: {}", e);
            anyhow::anyhow!("{}", e)
        })
}
