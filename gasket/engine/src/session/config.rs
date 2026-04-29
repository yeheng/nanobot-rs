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
pub const DEFAULT_MAX_ITERATIONS: u32 = 100;
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
/// Default tool execution timeout in seconds (2 minutes)
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;
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

/// Prompt templates — re-export from config layer (single source of truth).
pub use crate::config::app_config::PromptsConfig;

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
    /// Tool execution timeout in seconds
    pub tool_timeout_secs: u64,
    /// Subagent execution timeout in seconds
    pub subagent_timeout_secs: u64,
    /// Session idle timeout in seconds
    pub session_idle_timeout_secs: u64,
    /// Maximum characters for WebSocket subagent summary (0 = unlimited).
    pub ws_summary_limit: usize,
    /// Prompt configuration for internal AI behaviors.
    pub prompts: PromptsConfig,
    /// Memory token budget for three-phase context loading.
    pub memory_budget: Option<gasket_storage::wiki::MemoryBudget>,
    /// Self-evolution configuration (auto-learning from conversations).
    pub evolution: Option<EvolutionConfig>,
    /// Wiki knowledge system configuration (replaces memory system).
    pub wiki: Option<WikiConfig>,
    /// Optional path to an external stop-words file for keyword-based history recall.
    pub stop_words_path: Option<std::path::PathBuf>,
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
            tool_timeout_secs: DEFAULT_TOOL_TIMEOUT_SECS,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
            session_idle_timeout_secs: DEFAULT_SESSION_IDLE_TIMEOUT_SECS,
            ws_summary_limit: 0,
            prompts: PromptsConfig::default(),
            memory_budget: None,
            evolution: Some(EvolutionConfig::default()),
            wiki: None,
            stop_words_path: None,
        }
    }
}

/// Extension trait to convert AgentConfig → KernelConfig.
pub trait AgentConfigExt {
    fn to_kernel_config(&self) -> KernelConfig;
}

impl AgentConfigExt for AgentConfig {
    fn to_kernel_config(&self) -> KernelConfig {
        KernelConfig {
            model: self.model.clone(),
            max_iterations: self.max_iterations,
            max_retries: self.max_retries,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            max_tool_result_chars: self.max_tool_result_chars,
            thinking_enabled: self.thinking_enabled,
            tool_timeout_secs: self.tool_timeout_secs,
            ws_summary_limit: self.ws_summary_limit,
        }
    }
}
