//! Session configuration — AgentConfig with kernel conversion support.

use crate::kernel::KernelConfig;

fn default_wiki_base() -> String {
    dirs::home_dir()
        .map(|p| p.join(".gasket/wiki").to_str().unwrap().to_string())
        .unwrap_or_else(|| "~/.gasket/wiki".to_string())
}

fn default_sources_base() -> String {
    dirs::home_dir()
        .map(|p| p.join(".gasket/sources").to_str().unwrap().to_string())
        .unwrap_or_else(|| "~/.gasket/sources".to_string())
}

fn default_batch_size() -> usize {
    20
}

fn default_dedup_threshold() -> f64 {
    0.85
}

fn default_max_pages() -> usize {
    15
}

fn default_limit() -> usize {
    10
}

fn default_max_cost() -> f64 {
    0.10
}

fn default_cost_warning() -> f64 {
    0.05
}

fn default_lint_interval() -> String {
    "24h".to_string()
}

fn default_true() -> bool {
    true
}

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
pub const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 16000;
/// Default maximum retries for transient provider errors
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// Default subagent execution timeout in seconds (10 minutes)
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 600;
/// Default session idle timeout in seconds (1 hour)
pub const DEFAULT_SESSION_IDLE_TIMEOUT_SECS: u64 = 3600;
/// Default wait timeout for subagent results in seconds (12 minutes)
pub const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 720;

/// Configuration for the self-evolution hook.
#[derive(Clone, Debug)]
pub struct EvolutionConfig {
    /// Whether the evolution hook is enabled.
    pub enabled: bool,
    /// Number of messages to accumulate before triggering reflection.
    pub batch_messages: usize,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batch_messages: 20,
        }
    }
}

/// Wiki system configuration
#[derive(Clone, Debug)]
pub struct WikiConfig {
    pub enabled: bool,
    pub base_path: String,
    pub sources_path: String,
    pub ingest: WikiIngestConfig,
    pub query: WikiQueryConfig,
    pub lint: WikiLintConfig,
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_path: default_wiki_base(),
            sources_path: default_sources_base(),
            ingest: WikiIngestConfig::default(),
            query: WikiQueryConfig::default(),
            lint: WikiLintConfig::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WikiIngestConfig {
    pub batch_size: usize,
    pub auto_ingest: bool,
    pub dedup_threshold: f64,
    pub max_pages_per_ingest: usize,
    pub max_cost_per_ingest: f64,
    pub cost_warning_threshold: f64,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            auto_ingest: true,
            dedup_threshold: default_dedup_threshold(),
            max_pages_per_ingest: default_max_pages(),
            max_cost_per_ingest: default_max_cost(),
            cost_warning_threshold: default_cost_warning(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WikiQueryConfig {
    pub default_limit: usize,
    pub hybrid_search: bool,
    pub answer_filing: bool,
}

impl Default for WikiQueryConfig {
    fn default() -> Self {
        Self {
            default_limit: default_limit(),
            hybrid_search: true,
            answer_filing: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WikiLintConfig {
    pub enabled: bool,
    pub interval: String,
    pub auto_fix: bool,
    pub semantic_checks: bool,
}

impl Default for WikiLintConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            interval: default_lint_interval(),
            auto_fix: true,
            semantic_checks: true,
        }
    }
}

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
    /// Maximum retries for transient provider errors
    pub max_retries: u32,
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
    pub memory_budget: Option<gasket_storage::wiki::TokenBudget>,
    /// Self-evolution configuration (auto-learning from conversations).
    pub evolution: Option<EvolutionConfig>,
    /// Wiki knowledge system configuration (replaces memory system).
    pub wiki: Option<WikiConfig>,
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
            max_retries: DEFAULT_MAX_RETRIES,
            thinking_enabled: false,
            streaming: true,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
            session_idle_timeout_secs: DEFAULT_SESSION_IDLE_TIMEOUT_SECS,
            summarization_prompt: None,
            embedding_config: None,
            memory_budget: None,
            evolution: Some(EvolutionConfig::default()),
            wiki: None,
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
            .with_max_retries(self.max_retries)
            .with_temperature(self.temperature)
            .with_max_tokens(self.max_tokens)
            .with_thinking(self.thinking_enabled)
    }
}
