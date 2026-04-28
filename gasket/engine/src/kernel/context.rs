//! Kernel context: dependencies needed for the pure LLM execution loop.

use std::sync::Arc;

use crate::tools::{SubagentSpawner, ToolRegistry};
use async_trait::async_trait;
use gasket_providers::LlmProvider;

/// Async callback for proactive working-memory checkpoint injection.
///
/// Called before each `step()` with the current message count; returns
/// a summary to inject, or `None` to skip.
///
/// This is a kernel-level extension point (fires per iteration inside the
/// step loop), distinct from session-level `PipelineHook`s (fire once per
/// request during context construction).
#[async_trait]
pub trait CheckpointCallback: Send + Sync {
    async fn get_checkpoint(&self, msg_len: usize) -> Option<String>;
}

/// Everything the kernel needs to execute one LLM request.
/// Passed by reference to `kernel::execute()` — no ownership.
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    /// Optional checkpoint callback for proactive working-memory injection.
    /// `None` means no checkpointing — checked before each step iteration.
    pub checkpoint_callback: Option<Arc<dyn CheckpointCallback>>,
}

impl RuntimeContext {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            spawner: None,
            token_tracker: None,
            checkpoint_callback: None,
        }
    }
}

impl Clone for RuntimeContext {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: self.config.clone(),
            spawner: self.spawner.clone(),
            token_tracker: self.token_tracker.clone(),
            checkpoint_callback: self.checkpoint_callback.clone(),
        }
    }
}

/// Minimal config for the LLM iteration loop.
#[derive(Clone)]
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub max_retries: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
    pub tool_timeout_secs: u64,
}

impl KernelConfig {
    pub fn new(model: String) -> Self {
        Self {
            model,
            max_iterations: 100,
            max_retries: 3,
            temperature: 1.0,
            max_tokens: 65536,
            max_tool_result_chars: 16000,
            thinking_enabled: false,
            tool_timeout_secs: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_config_default() {
        let config = KernelConfig::new("test-model".to_string());
        assert_eq!(config.model, "test-model");
        assert_eq!(config.max_iterations, 100);
        assert_eq!(config.max_retries, 3);
    }
}
