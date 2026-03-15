//! Agent loop: the core processing engine
//!
//! ## Execution Flow
//!
//! The main pipeline in 'process_direct_with_callback' is a straight-line sequence:
//!
//! 1. external_hook(pre_request)  → shell script can abort or modify input
//! 2. load_session                → context.load_session() (trait dispatch)
//! 3. save_user_message           → context.save_message() (trait dispatch)
//! 4. process_history             → pure: truncate history, compute evictions
//! 5. load_summary + bg_compress  → load existing summary (fast), spawn background compression if messages were evicted (non-blocking)
//! 6. inject_system_prompts       → direct: bootstrap + skills
//! 7. assemble_prompt             → pure: build Vec<ChatMessage>
//!    7.5. vault_injection            → inject secrets from vault (optional)
//! 8. run_agent_loop              → LLM iteration (with inline logging)
//! 9. external_hook(post_response) → shell script for audit/alerting
//! 10. save_assistant_msg          → context.save_message() (trait dispatch)
//!
//! All steps are **direct method calls** or pure functions — no hidden hook dispatch.
//! External shell hooks (if present) are called via subprocess at steps 1 and 10.
//! Step 5's background compression uses `tokio::spawn` — zero user-facing latency.
//! Step 7.5 injects vault secrets directly via `VaultInjector`.
//!
//! ## AgentContext Trait Pattern
//!
//! The agent uses the `AgentContext` trait for state management, eliminating
//! `Option<T>` checks in the core loop:
//! - **PersistentContext** — for main agents with full persistence
//! - **StatelessContext** — for subagents without persistence
//!
//! This pattern allows polymorphic dispatch at initialization time rather than
//! runtime branching on every message.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::context::AgentContext;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::prompt;
use crate::agent::stream::{self};
use crate::bus::events::SessionKey;
use crate::error::AgentError;
use crate::hooks::ExternalHookRunner;
use crate::providers::{ChatMessage, LlmProvider};
use crate::tools::ToolRegistry;
use crate::vault::{redact_secrets, VaultInjector, VaultStore};

use crate::agent::context::{PersistentContext, StatelessContext};
use crate::agent::memory::MemoryStore;
use crate::agent::summarization::SummarizationService;
use crate::session::SessionManager;
use std::sync::Mutex;

/// Default model for agent
const DEFAULT_MODEL: &str = "gpt-4o";
/// Default maximum iterations for agent loop
const DEFAULT_MAX_ITERATIONS: u32 = 20;
/// Default temperature for generation
const DEFAULT_TEMPERATURE: f32 = 1.0;
/// Default maximum tokens for generation
const DEFAULT_MAX_TOKENS: u32 = 65536;
/// Default memory window size
const DEFAULT_MEMORY_WINDOW: usize = 50;
/// Default maximum characters for tool result output
const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 8000;

/// Agent loop configuration
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
        }
    }
}

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// Main response content
    pub content: String,
    /// Reasoning/thinking content (if thinking mode enabled)
    pub reasoning_content: Option<String>,
    /// Tools used during processing
    pub tools_used: Vec<String>,
    /// Model name used for this response
    pub model: Option<String>,
}

/// Result from the agent loop execution.
#[derive(Debug)]
#[allow(dead_code)]
struct AgentLoopResult {
    /// Main response content
    content: String,
    /// Reasoning/thinking content (if thinking mode enabled)
    reasoning_content: Option<String>,
    /// Tools used during processing
    tools_used: Vec<String>,
    /// Token usage for this request
    token_usage: Option<crate::token_tracker::TokenUsage>,
    /// Cost for this request
    cost: f64,
}

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// Uses **AgentContext trait** for state management (polymorphic dispatch):
/// - **PersistentContext** — main agents with session persistence and compression
/// - **StatelessContext** — subagents without persistence
///
/// This pattern eliminates `Option<T>` checks in the hot path — the context
/// is determined at initialization, not at every message.
///
/// Explicit long-term memory lives in `~/.nanobot/memory/*.md` files (SSOT).
/// SQLite only stores machine-state (sessions, summaries, cron, kv).
///
/// System prompt and skills context are loaded **once** at initialization
/// and stored as plain 'String' fields — no dynamic dispatch.
///
/// External shell hooks ('~/.nanobot/hooks/') are invoked at request
/// boundaries (pre_request / post_response) via subprocess — UNIX philosophy.
///
/// Message interceptors (e.g., VaultInjector) are called before LLM processing
/// directly — no middleware chain indirection.
///
/// ## Security Note: Vault Values Lifecycle
///
/// Injected vault values (plaintext secrets) are scoped to **single requests**.
/// They are collected as local variables in `process_direct_with_callback`,
/// passed through the agent loop, and dropped when the request completes.
/// This prevents memory accumulation and limits exposure window.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: AgentConfig,
    workspace: PathBuf,
    /// History truncator configuration.
    history_config: HistoryConfig,
    /// Agent context — handles session persistence and compression.
    /// Uses trait dispatch instead of Option<T> to eliminate branching.
    context: Arc<dyn AgentContext>,
    /// Pre-loaded system prompt (from workspace bootstrap files).
    system_prompt: String,
    /// Pre-loaded skills context (from workspace skills).
    skills_context: Option<String>,
    /// External shell hook runner (pre_request / post_response).
    external_hooks: ExternalHookRunner,
    /// Vault injector for pre-LLM secret injection (optional)
    vault_injector: Option<VaultInjector>,
    /// Pricing configuration for cost calculation (optional)
    pricing: Option<crate::token_tracker::ModelPricing>,
    /// Token usage tracker for the session
    session_stats: Mutex<crate::token_tracker::SessionTokenStats>,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    ///
    /// Loads system prompt and skills context **once** at initialization.
    /// Logging is inlined directly — no hook indirection.
    /// External shell hooks are loaded from '~/.nanobot/hooks/'.
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let memory_store = Arc::new(MemoryStore::new().await);
        Self::with_services(provider, workspace, config, tools, memory_store).await
    }

    /// Internal helper: create AgentLoop with pre-created services.
    async fn with_services(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
    ) -> Result<Self, AgentError> {
        let session_manager = Arc::new(SessionManager::new(memory_store.sqlite_store().clone()));

        let store_arc = memory_store.sqlite_store().clone();
        let summarization = Arc::new(SummarizationService::new(
            provider.clone(),
            Arc::new(store_arc),
            config.model.clone(),
        ));

        let context: Arc<dyn AgentContext> =
            Arc::new(PersistentContext::new(session_manager, summarization));

        let (system_prompt, skills_context) = Self::load_prompts(&workspace).await?;
        let external_hooks = Self::load_external_hooks();
        let vault_injector = Self::create_vault_injector();

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            external_hooks,
            vault_injector,
            pricing: None,
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new("USD")),
        })
    }

    /// Load system prompt and skills context from workspace.
    async fn load_prompts(workspace: &Path) -> Result<(String, Option<String>), AgentError> {
        let system_prompt =
            prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(workspace).await;
        Ok((system_prompt, skills_context))
    }

    /// Create external hook runner from ~/.nanobot/hooks/.
    fn load_external_hooks() -> ExternalHookRunner {
        let hooks_dir = dirs::home_dir()
            .map(|p| p.join(".nanobot").join("hooks"))
            .unwrap_or_else(|| {
                tracing::warn!("Could not resolve home directory, disabling external hooks.");
                PathBuf::from("/dev/null")
            });
        ExternalHookRunner::new(hooks_dir)
    }

    /// Initialize vault injector if VaultStore is available.
    fn create_vault_injector() -> Option<VaultInjector> {
        match VaultStore::new() {
            Ok(store) => {
                debug!("[Agent] Vault initialized successfully, adding vault injector");
                Some(VaultInjector::new(Arc::new(store)))
            }
            Err(_) => {
                debug!("[Agent] Vault not available, skipping vault injector");
                None
            }
        }
    }

    /// Create a new agent loop with an **externally created** 'MemoryStore'.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    /// Use this when the 'MemoryStore' must be shared with other components.
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub async fn with_memory_store(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        memory_store: Arc<MemoryStore>,
    ) -> Result<Self, AgentError> {
        Self::with_memory_store_and_pricing(provider, workspace, config, tools, memory_store, None)
            .await
    }

    /// Create a new agent loop with MemoryStore and pricing configuration.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    pub async fn with_memory_store_and_pricing(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        memory_store: Arc<MemoryStore>,
        pricing: Option<crate::token_tracker::ModelPricing>,
    ) -> Result<Self, AgentError> {
        let session_manager = Arc::new(SessionManager::new(memory_store.sqlite_store().clone()));

        let store_arc = memory_store.sqlite_store().clone();
        let summarization = Arc::new(SummarizationService::new(
            provider.clone(),
            Arc::new(store_arc),
            config.model.clone(),
        ));

        // Create persistent context for main agents
        let context: Arc<dyn AgentContext> =
            Arc::new(PersistentContext::new(session_manager, summarization));

        // Load system prompt and skills directly — no hook indirection
        let system_prompt =
            prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(&workspace).await;

        // External shell hooks: look for scripts in ~/.nanobot/hooks/
        let hooks_dir = dirs::home_dir()
            .map(|p| p.join(".nanobot").join("hooks"))
            .unwrap_or_else(|| {
                tracing::warn!("Could not resolve home directory, disabling external hooks.");
                PathBuf::from("/dev/null")
            });
        let external_hooks = ExternalHookRunner::new(hooks_dir);

        // Initialize vault injector (optional - for sensitive data isolation)
        let vault_injector = match VaultStore::new() {
            Ok(store) => {
                debug!("[Agent] Vault initialized successfully, adding vault injector");
                Some(VaultInjector::new(Arc::new(store)))
            }
            Err(_) => {
                debug!("[Agent] Vault not available, skipping vault injector");
                None
            }
        };

        Ok(Self {
            provider,
            tools: Arc::new(tools),
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            external_hooks,
            vault_injector,
            pricing: pricing.clone(),
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new(
                pricing
                    .as_ref()
                    .map(|p| p.currency.as_str())
                    .unwrap_or("USD"),
            )),
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// Uses **StatelessContext** — no persistence, all operations are no-ops.
    /// System prompt is empty by default; use 'set_system_prompt()' to configure.
    /// No external hooks for subagents.
    /// No vault for subagents (empty interceptor chain).
    pub fn builder(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        // Use stateless context for subagents
        let context: Arc<dyn AgentContext> = Arc::new(StatelessContext::new());

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt: String::new(),
            skills_context: None,
            external_hooks: ExternalHookRunner::noop(),
            vault_injector: None, // No vault for subagents
            pricing: None,
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new("USD")),
        })
    }

    /// Set the system prompt (used by subagents to configure identity).
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Get the model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Clear the session for the given key (used by CLI for '/new' command).
    ///
    /// This resets the conversation history so the next message starts fresh.
    /// For stateless contexts (subagents), this is a no-op.
    pub async fn clear_session(&self, session_key: &SessionKey) {
        // Only clear if we have persistence
        if self.context.is_persistent() {
            self.context.load_session(session_key).await;
            // Note: SessionManager has a clear_session method, but AgentContext
            // doesn't expose it directly. For now, we skip this optimization
            // since the session will be fresh on next load anyway.
        }
    }

    /// Process a message and return response.
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        let session_key_str = session_key.to_string();
        // ── 1. External hook: pre_request (can abort or modify) ───
        let content = match self
            .external_hooks
            .run_pre_request(&session_key_str, content)
            .await
        {
            Ok(Some(output)) => {
                if output.is_abort() {
                    let error_msg = output.error.unwrap_or_else(|| "请求被拒绝".to_string());
                    return Ok(AgentResponse {
                        content: error_msg,
                        reasoning_content: None,
                        tools_used: vec![],
                        model: Some(self.config.model.clone()),
                    });
                }
                // Use modified message if provided, otherwise keep original
                output
                    .modified_message
                    .unwrap_or_else(|| content.to_string())
            }
            Ok(None) => content.to_string(), // No hook or empty output — continue
            Err(e) => {
                warn!("pre_request hook failed (continuing): {}", e);
                content.to_string()
            }
        };
        let content = content.as_str();

        // ── 2. Load session history (trait dispatch) ─────
        let session = self.context.load_session(session_key).await;
        let history_snapshot = session.get_history(self.config.memory_window);

        // ── 3. Save user message (trait dispatch) ────────────────
        self.context
            .save_message(session_key, "user", content, None)
            .await;

        // ── 4. Truncate history (pure computation) ─────────────────
        let processed = process_history(history_snapshot, &self.history_config);

        // ── 5. Load existing summary + spawn background compression ─────
        //
        // The summarization LLM call is expensive (~10-30s). Instead of blocking
        // the user's response, we:
        //   a) Load the existing (possibly stale) summary — cheap SQLite read.
        //   b) If there are evictions, fire off background compression.
        //   c) Use the existing summary for this turn's prompt assembly.
        let summary = self.context.load_summary(&session_key_str).await;

        // If messages were evicted, trigger background compression (non-blocking)
        if !processed.evicted.is_empty() {
            self.context
                .compress_context(&session_key_str, &processed.evicted);
        }

        // ── 6. Inject system prompts (direct) ──────────────────────
        let mut system_prompts = Vec::new();
        if !self.system_prompt.is_empty() {
            system_prompts.push(self.system_prompt.clone());
        }
        if let Some(ref skills) = self.skills_context {
            system_prompts.push(skills.clone());
        }

        // ── 7. Assemble prompt (pure, synchronous) ─────────────────
        let mut messages = Self::assemble_prompt(
            processed.messages,
            content,
            &system_prompts,
            summary.as_deref(),
        );

        // ── 7.5. Vault injection ────────────────────────────────────
        // Inject secrets at the last moment before sending to LLM.
        // CRITICAL: Collect injected values into LOCAL variable for log redaction!
        // This prevents memory leaks by scoping secrets to single requests.
        let mut local_vault_values = Vec::new();
        if let Some(ref vault) = self.vault_injector {
            let report = vault.inject(&mut messages);
            local_vault_values = report.injected_values;
            if !report.keys_used.is_empty() {
                debug!(
                    "[Agent] Vault injected {} keys into {} messages",
                    report.keys_used.len(),
                    report.messages_modified
                );
            }
        }
        // Deduplicate
        local_vault_values.sort();
        local_vault_values.dedup();
        if !local_vault_values.is_empty() {
            debug!(
                "[Agent] Collected {} injected values for log redaction (scoped to this request)",
                local_vault_values.len()
            );
        }

        // ── 8. Run agent loop ─────────────────────────────────────
        let result = self.run_agent_loop(messages, &local_vault_values).await?;

        // ── 9. External hook: post_response (audit / alerting) ────
        let tools_used_str = result.tools_used.join(", ");

        // Redact secrets from post_response hook
        let safe_content = redact_secrets(&result.content, &local_vault_values);

        if let Err(e) = self
            .external_hooks
            .run_post_response(&session_key_str, &safe_content, &tools_used_str)
            .await
        {
            warn!("post_response hook failed (ignored): {}", e);
        }

        // ── 10. Save assistant message (trait dispatch) ──────────
        // Redact secrets before saving to history
        let history_content = redact_secrets(&result.content, &local_vault_values);

        self.context
            .save_message(
                session_key,
                "assistant",
                &history_content,
                Some(result.tools_used.clone()),
            )
            .await;

        // Output session token stats if available
        if let Ok(stats) = self.session_stats.lock() {
            if stats.request_count > 0 {
                info!("{}", stats.format_summary());
            }
        }

        Ok(AgentResponse {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
            model: Some(self.config.model.clone()),
        })
    }

    /// Process a message with streaming callback.
    pub async fn process_direct_streaming<F>(
        &self,
        content: &str,
        session_key: &SessionKey,
        callback: F,
    ) -> Result<AgentResponse, AgentError>
    where
        F: FnMut(stream::StreamEvent) + Send + 'static,
    {
        let session_key_str = session_key.to_string();
        let content = match self
            .external_hooks
            .run_pre_request(&session_key_str, content)
            .await
        {
            Ok(Some(output)) => {
                if output.is_abort() {
                    let error_msg = output.error.unwrap_or_else(|| "请求被拒绝".to_string());
                    return Ok(AgentResponse {
                        content: error_msg,
                        reasoning_content: None,
                        tools_used: vec![],
                        model: Some(self.config.model.clone()),
                    });
                }
                output
                    .modified_message
                    .unwrap_or_else(|| content.to_string())
            }
            Ok(None) => content.to_string(),
            Err(e) => {
                warn!("pre_request hook failed (continuing): {}", e);
                content.to_string()
            }
        };
        let content = content.as_str();

        let session = self.context.load_session(session_key).await;
        let history_snapshot = session.get_history(self.config.memory_window);
        self.context
            .save_message(session_key, "user", content, None)
            .await;

        let processed = process_history(history_snapshot, &self.history_config);
        let summary = self.context.load_summary(&session_key_str).await;
        if !processed.evicted.is_empty() {
            self.context
                .compress_context(&session_key_str, &processed.evicted);
        }

        let mut system_prompts = Vec::new();
        if !self.system_prompt.is_empty() {
            system_prompts.push(self.system_prompt.clone());
        }
        if let Some(ref skills) = self.skills_context {
            system_prompts.push(skills.clone());
        }

        let mut messages = Self::assemble_prompt(
            processed.messages,
            content,
            &system_prompts,
            summary.as_deref(),
        );

        let mut local_vault_values = Vec::new();
        if let Some(ref vault) = self.vault_injector {
            let report = vault.inject(&mut messages);
            local_vault_values = report.injected_values;
        }
        local_vault_values.sort();
        local_vault_values.dedup();

        let result = self
            .run_agent_loop_streaming(messages, &local_vault_values, callback)
            .await?;

        let tools_used_str = result.tools_used.join(", ");
        let safe_content = redact_secrets(&result.content, &local_vault_values);
        if let Err(e) = self
            .external_hooks
            .run_post_response(&session_key_str, &safe_content, &tools_used_str)
            .await
        {
            warn!("post_response hook failed (ignored): {}", e);
        }

        let history_content = redact_secrets(&result.content, &local_vault_values);
        self.context
            .save_message(
                session_key,
                "assistant",
                &history_content,
                Some(result.tools_used.clone()),
            )
            .await;

        if let Ok(stats) = self.session_stats.lock() {
            if stats.request_count > 0 {
                info!("{}", stats.format_summary());
            }
        }

        Ok(AgentResponse {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
            model: Some(self.config.model.clone()),
        })
    }

    // ── Agent Loop Internals ────────────────────────────────
    // Note: calculate_token_usage and handle_tool_calls were moved to executor_core.rs
    // as part of the AgentExecutor refactoring.

    /// Unified agent iteration loop.
    ///
    /// Delegates to `AgentExecutor` for the core LLM loop.
    /// Handles session stats tracking after execution completes.
    ///
    /// # Security: Vault Values Scoping
    ///
    /// `vault_values` is passed as a parameter (not stored in self) to ensure
    /// plaintext secrets are scoped to single requests. This prevents memory
    /// accumulation and limits the exposure window for sensitive data.
    async fn run_agent_loop(
        &self,
        messages: Vec<ChatMessage>,
        vault_values: &[String],
    ) -> Result<AgentLoopResult, AgentError> {
        use crate::agent::executor_core::{AgentExecutor, ExecutorOptions};

        let executor = AgentExecutor::new(self.provider.clone(), self.tools.clone(), &self.config);

        let mut options = ExecutorOptions::new().with_vault_values(vault_values);
        if let Some(ref pricing) = self.pricing {
            options = options.with_pricing(pricing.clone());
        }

        let result = executor.execute_with_options(messages, &options).await?;

        // Update session stats
        if let Some(ref usage) = result.token_usage {
            if let Ok(mut stats) = self.session_stats.lock() {
                stats.add_usage(usage, result.cost);
            }
        }

        Ok(AgentLoopResult {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
            token_usage: result.token_usage,
            cost: result.cost,
        })
    }

    /// Execute with streaming callback.
    ///
    /// Delegates to `AgentExecutor` for the core LLM loop with streaming.
    /// Handles session stats tracking after execution completes.
    ///
    /// # Security: Vault Values Scoping
    ///
    /// `vault_values` is passed as a parameter (not stored in self) to ensure
    /// plaintext secrets are scoped to single requests.
    async fn run_agent_loop_streaming<F>(
        &self,
        messages: Vec<ChatMessage>,
        vault_values: &[String],
        mut callback: F,
    ) -> Result<AgentLoopResult, AgentError>
    where
        F: FnMut(stream::StreamEvent) + Send + 'static,
    {
        use crate::agent::executor_core::{AgentExecutor, ExecutorOptions};
        use tokio::sync::mpsc;

        let executor = AgentExecutor::new(self.provider.clone(), self.tools.clone(), &self.config);

        let mut options = ExecutorOptions::new().with_vault_values(vault_values);
        if let Some(ref pricing) = self.pricing {
            options = options.with_pricing(pricing.clone());
        }

        // Create channel for stream events
        let (event_tx, mut event_rx) = mpsc::channel(64);

        // Spawn task to forward events to callback
        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                callback(event);
            }
        });

        // Execute with streaming
        let result = executor
            .execute_stream_with_options(messages, event_tx, &options)
            .await?;

        // Wait for event forwarding to complete
        let _ = forward_handle.await;

        // Update session stats
        if let Some(ref usage) = result.token_usage {
            if let Ok(mut stats) = self.session_stats.lock() {
                stats.add_usage(usage, result.cost);
            }
        }

        Ok(AgentLoopResult {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
            token_usage: result.token_usage,
            cost: result.cost,
        })
    }

    // Note: handle_tool_calls was moved to executor_core.rs as part of the AgentExecutor refactoring.
}

// ── Helpers ─────────────────────────────────────────────────

impl AgentLoop {
    /// Pure, synchronous assembly of the LLM prompt sequence.
    fn assemble_prompt(
        processed_history: Vec<crate::session::SessionMessage>,
        current_message: &str,
        system_prompts: &[String],
        summary: Option<&str>,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // 1. Build the system prompt (only if non-empty)
        if !system_prompts.is_empty() {
            let system_content = system_prompts.join("\n\n");
            if !system_content.is_empty() {
                messages.push(ChatMessage::system(system_content));
            }
        }

        // 2. Inject summary as assistant message (if exists)
        if let Some(summary_text) = summary {
            if !summary_text.is_empty() {
                messages.push(ChatMessage::assistant(format!(
                    "{}{}",
                    crate::agent::summarization::SUMMARY_PREFIX,
                    summary_text
                )));
            }
        }

        // 3. Add processed history messages (consume msg.content to avoid cloning)
        let history_count = processed_history.len();
        for msg in processed_history {
            match msg.role {
                crate::providers::MessageRole::User => {
                    messages.push(ChatMessage::user(msg.content))
                }
                crate::providers::MessageRole::Assistant => {
                    messages.push(ChatMessage::assistant(msg.content))
                }
                _ => {}
            }
        }

        // 4. Current message
        messages.push(ChatMessage::user(current_message));

        debug!(
            "Built messages: {} history msgs, summary: {}",
            history_count,
            summary.is_some()
        );

        messages
    }
}
