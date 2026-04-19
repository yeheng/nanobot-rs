//! Kernel context: dependencies needed for the pure LLM execution loop.

use std::sync::Arc;

use crate::tools::{SubagentSpawner, ToolRegistry};
use gasket_providers::LlmProvider;

/// Everything the kernel needs to execute one LLM request.
/// Passed by reference to `kernel::execute()` — no ownership.
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}

impl Clone for RuntimeContext {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: self.config.clone(),
            spawner: self.spawner.clone(),
            token_tracker: self.token_tracker.clone(),
        }
    }
}

/// Minimal config for the LLM iteration loop.
/// `#[non_exhaustive]` prevents external crates from adding fields
/// — only LLM loop parameters belong here.
#[non_exhaustive]
#[derive(Clone)]
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub max_retries: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
}

impl KernelConfig {
    pub fn new(model: String) -> Self {
        Self {
            model,
            max_iterations: 20,
            max_retries: 3,
            temperature: 1.0,
            max_tokens: 65536,
            max_tool_result_chars: 16000,
            thinking_enabled: false,
        }
    }

    pub fn with_max_iterations(mut self, v: u32) -> Self {
        self.max_iterations = v;
        self
    }
    pub fn with_max_retries(mut self, v: u32) -> Self {
        self.max_retries = v;
        self
    }
    pub fn with_temperature(mut self, v: f32) -> Self {
        self.temperature = v;
        self
    }
    pub fn with_max_tokens(mut self, v: u32) -> Self {
        self.max_tokens = v;
        self
    }
    pub fn with_thinking(mut self, v: bool) -> Self {
        self.thinking_enabled = v;
        self
    }
}
