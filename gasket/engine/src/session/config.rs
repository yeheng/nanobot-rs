//! Session configuration — AgentConfig with kernel conversion support.

use crate::kernel::KernelConfig;

/// Default model for agent
pub const DEFAULT_MODEL: &str = "gpt-4o";
/// Default maximum iterations for agent loop
pub const DEFAULT_MAX_ITERATIONS: u32 = 20;
/// Default temperature for generation
pub const DEFAULT_TEMPERATURE: f32 = 1.0;
/// Default maximum tokens for generation
pub const DEFAULT_MAX_TOKENS: u32 = 65536;
/// Default memory window size
pub const DEFAULT_MEMORY_WINDOW: usize = 50;
/// Default maximum characters for tool result output
pub const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 8000;
/// Default subagent execution timeout in seconds (10 minutes)
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 600;
/// Default session idle timeout in seconds (1 hour)
pub const DEFAULT_SESSION_IDLE_TIMEOUT_SECS: u64 = 3600;
/// Default wait timeout for subagent results in seconds (12 minutes)
pub const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 720;

/// Agent loop configuration
#[derive(Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub memory_window: usize,
    /// Maximum characters for tool result output (0 = unlimited)
    pub max_tool_result_chars: usize,
    /// Enable thinking/reasoning mode for deep reasoning models
    pub thinking_enabled: bool,
    /// Enable streaming mode for progressive output
    pub streaming: bool,
    /// Subagent execution timeout in seconds
    pub subagent_timeout_secs: u64,
    /// Session idle timeout in seconds
    pub session_idle_timeout_secs: u64,
    /// Custom summarization prompt (overrides built-in default).
    /// When set, this prompt is used by ContextCompactor to generate summaries.
    pub summarization_prompt: Option<String>,
    /// Embedding configuration for semantic search and memory indexing.
    pub embedding_config: Option<crate::config::EmbeddingConfig>,
    /// Memory token budget for three-phase context loading.
    pub memory_budget: Option<gasket_storage::memory::TokenBudget>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
            memory_window: DEFAULT_MEMORY_WINDOW,
            max_tool_result_chars: DEFAULT_MAX_TOOL_RESULT_CHARS,
            thinking_enabled: false,
            streaming: true,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
            session_idle_timeout_secs: DEFAULT_SESSION_IDLE_TIMEOUT_SECS,
            summarization_prompt: None,
            embedding_config: None,
            memory_budget: None,
        }
    }
}

/// Extension trait to convert AgentConfig → KernelConfig.
pub trait AgentConfigExt {
    fn to_kernel_config(&self) -> KernelConfig;
}

impl AgentConfigExt for AgentConfig {
    fn to_kernel_config(&self) -> KernelConfig {
        KernelConfig::new(self.model.clone())
            .with_max_iterations(self.max_iterations)
            .with_temperature(self.temperature)
            .with_max_tokens(self.max_tokens)
            .with_thinking(self.thinking_enabled)
    }
}
