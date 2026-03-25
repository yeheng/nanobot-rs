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
//!    7.6. history_recall              → semantic recall of old messages (optional)
//! 8. run_agent_loop              → LLM iteration (with inline logging)
//! 9. external_hook(post_response) → shell script for audit/alerting
//! 10. save_assistant_msg          → context.save_message() (trait dispatch)
//!
//! All steps are **direct method calls** or pure functions — no hidden hook dispatch.
//! External shell hooks (if present) are called via subprocess at steps 1 and 10.
//! Step 5's background compression uses `tokio::spawn` — zero user-facing latency.
//! Step 7.5 injects vault secrets directly via `VaultInjector`.
//! Step 7.6 recalls relevant history via semantic embedding search.
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
use crate::hooks::{
    ExternalHookRunner, ExternalShellHook, HookAction, HookBuilder, HookPoint, HookRegistry,
    MutableContext, VaultHook,
};
use crate::providers::{ChatMessage, LlmProvider};
use crate::tools::ToolRegistry;
use crate::vault::{redact_secrets, VaultInjector, VaultStore};
use tokio::sync::RwLock;

use crate::agent::context::{PersistentContext, StatelessContext};
use crate::agent::memory::MemoryStore;
use crate::agent::summarization::SummarizationService;
use crate::session::SessionManager;

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
    /// Token usage for this request (if tracking enabled)
    pub token_usage: Option<crate::token_tracker::TokenUsage>,
    /// Cost for this request (if pricing configured)
    pub cost: f64,
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
/// Explicit long-term memory lives in `~/.gasket/memory/*.md` files (SSOT).
/// SQLite only stores machine-state (sessions, summaries, cron, kv).
///
/// System prompt and skills context are loaded **once** at initialization
/// and stored as plain 'String' fields — no dynamic dispatch.
///
/// Lifecycle hooks are managed via **HookRegistry** — a unified mechanism for:
/// - External shell hooks (pre_request / post_response)
/// - Vault secret injection (before LLM)
/// - Semantic history recall (after history load)
///
/// ## Hook Architecture
///
/// The hook registry provides a unified interface for all pipeline hooks:
/// - `BeforeRequest`: External shell hooks can modify/abort requests
/// - `AfterHistory`: Semantic recall of relevant context
/// - `BeforeLLM`: Vault secret injection
/// - `AfterResponse`: External shell hooks for auditing
///
/// ## Security Note: Vault Values Lifecycle
///
/// Injected vault values (plaintext secrets) are scoped to **single requests**.
/// They are collected in `vault_values` field, passed through the agent loop,
/// and used for log redaction. They persist across the request lifetime.
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
    /// Unified hook registry for all lifecycle hooks.
    /// Replaces external_hooks, vault_injector, embedder, history_recall_k.
    hooks: Arc<HookRegistry>,
    /// Pricing configuration for cost calculation (optional)
    pricing: Option<crate::token_tracker::ModelPricing>,
    /// Injected vault values for log redaction (shared with VaultHook)
    vault_values: Arc<RwLock<Vec<String>>>,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    ///
    /// Loads system prompt and skills context **once** at initialization.
    /// Logging is inlined directly — no hook indirection.
    /// External shell hooks are loaded from '~/.gasket/hooks/'.
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

        // Build hooks using HookBuilder
        let (hooks, vault_values) = Self::build_hooks();

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            hooks,
            pricing: None,
            vault_values,
        })
    }

    /// Load system prompt and skills context from workspace.
    async fn load_prompts(workspace: &Path) -> Result<(String, Option<String>), AgentError> {
        let system_prompt =
            prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(workspace).await;
        Ok((system_prompt, skills_context))
    }

    /// Build hooks registry for main agents.
    ///
    /// Creates:
    /// - ExternalShellHook at BeforeRequest and AfterResponse
    /// - VaultHook at BeforeLLM (if vault is available)
    fn build_hooks() -> (Arc<HookRegistry>, Arc<RwLock<Vec<String>>>) {
        let vault_values = Arc::new(RwLock::new(Vec::new()));

        let hooks_dir = dirs::home_dir()
            .map(|p| p.join(".gasket").join("hooks"))
            .unwrap_or_else(|| {
                tracing::warn!("Could not resolve home directory, disabling external hooks.");
                PathBuf::from("/dev/null")
            });

        let external_runner = ExternalHookRunner::new(hooks_dir);

        let mut builder = HookBuilder::new()
            // External shell hooks at BeforeRequest and AfterResponse
            .with_hook(Arc::new(ExternalShellHook::new(
                external_runner.clone(),
                HookPoint::BeforeRequest,
            )))
            .with_hook(Arc::new(ExternalShellHook::new(
                external_runner,
                HookPoint::AfterResponse,
            )));

        // Add vault hook if available
        let vault_values = if let Ok(store) = VaultStore::new() {
            debug!("[Agent] Vault initialized successfully, adding vault injector");
            let vault_hook = VaultHook::new(VaultInjector::new(Arc::new(store)));
            // Get the injected values handle for log redaction
            let values = vault_hook.injected_values();
            builder = builder.with_hook(Arc::new(vault_hook));
            values
        } else {
            debug!("[Agent] Vault not available, skipping vault injector");
            vault_values
        };

        (builder.build_shared(), vault_values)
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

        // Build hooks using unified registry
        let (hooks, vault_values) = Self::build_hooks();

        Ok(Self {
            provider,
            tools: Arc::new(tools),
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            hooks,
            pricing,
            vault_values,
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// Uses **StatelessContext** — no persistence, all operations are no-ops.
    /// System prompt is empty by default; use 'set_system_prompt()' to configure.
    /// No hooks for subagents by default; use 'with_hooks()' to customize.
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
            hooks: HookRegistry::empty(), // Empty hooks for subagents
            pricing: None,
            vault_values: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Set the system prompt (used by subagents to configure identity).
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Set custom hooks for this agent.
    ///
    /// Used by subagents to inherit hooks from the main agent.
    pub fn with_hooks(mut self, hooks: Arc<HookRegistry>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Get a reference to the hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Get mutable access to vault values (for log redaction).
    pub fn vault_values(&self) -> &Arc<RwLock<Vec<String>>> {
        &self.vault_values
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

        // ── 1. Build initial mutable context for hooks ─────────────
        let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(content)];
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(content),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        // ── 2. BeforeRequest hooks (can modify input or abort) ─────
        match self.hooks.execute(HookPoint::BeforeRequest, &mut ctx).await? {
            HookAction::Abort(msg) => {
                return Ok(AgentResponse {
                    content: msg,
                    reasoning_content: None,
                    tools_used: vec![],
                    model: Some(self.config.model.clone()),
                    token_usage: None,
                    cost: 0.0,
                });
            }
            HookAction::Continue => {}
        }

        // Get the (possibly modified) user content
        let content: String = ctx
            .messages
            .iter()
            .find(|m| m.role == crate::providers::MessageRole::User)
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| content.to_string());

        // ── 3. Load session history (trait dispatch) ─────
        let session = self.context.load_session(session_key).await;
        let history_snapshot = session.get_history(self.config.memory_window);

        // ── 4. Save user message (trait dispatch) ────────────────
        self.context
            .save_message(session_key, "user", &content, None)
            .await;

        // ── 5. Truncate history (pure computation) ─────────────────
        let processed = process_history(history_snapshot, &self.history_config);

        // ── 6. Load existing summary + spawn background compression ─────
        let summary = self.context.load_summary(&session_key_str).await;
        if !processed.evicted.is_empty() {
            self.context
                .compress_context(&session_key_str, &processed.evicted);
        }

        // ── 7. Inject system prompts (direct) ──────────────────────
        let mut system_prompts = Vec::new();
        if !self.system_prompt.is_empty() {
            system_prompts.push(self.system_prompt.clone());
        }
        if let Some(ref skills) = self.skills_context {
            system_prompts.push(skills.clone());
        }

        // ── 8. Assemble prompt (pure, synchronous) ─────────────────
        let mut messages = Self::assemble_prompt(
            processed.messages,
            &content,
            &system_prompts,
            summary.as_deref(),
            None, // History recall is now handled by hooks
        );

        // ── 9. AfterHistory hooks (semantic recall, etc.) ───────────
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(&content),
            response: None,
            tool_calls: None,
            token_usage: None,
        };
        self.hooks.execute(HookPoint::AfterHistory, &mut ctx).await?;

        // ── 10. BeforeLLM hooks (vault injection, etc.) ────────────
        self.hooks.execute(HookPoint::BeforeLLM, &mut ctx).await?;

        // Get vault values for log redaction
        let local_vault_values = self.vault_values.read().await.clone();

        // ── 11. Run agent loop ─────────────────────────────────────
        let result = self.run_agent_loop(messages, &local_vault_values).await?;

        // ── 12. AfterResponse hooks (audit, logging, etc.) ────────
        let tools_used: Vec<crate::hooks::ToolCallInfo> = result
            .tools_used
            .iter()
            .map(|name| crate::hooks::ToolCallInfo {
                id: name.clone(),
                name: name.clone(),
                arguments: None,
            })
            .collect();

        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut vec![],
            user_input: Some(&content),
            response: Some(&result.content),
            tool_calls: Some(&tools_used),
            token_usage: result.token_usage.as_ref(),
        };
        self.hooks.execute(HookPoint::AfterResponse, &mut ctx).await?;

        // ── 13. Save assistant message (trait dispatch) ──────────
        let history_content = redact_secrets(&result.content, &local_vault_values);
        self.context
            .save_message(
                session_key,
                "assistant",
                &history_content,
                Some(result.tools_used.clone()),
            )
            .await;

        // Log token usage if available
        if let Some(ref usage) = result.token_usage {
            info!(
                "[Token] Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
                usage.input_tokens, usage.output_tokens, usage.total_tokens, result.cost
            );
        }

        Ok(AgentResponse {
            content: result.content.clone(),
            reasoning_content: result.reasoning_content.clone(),
            tools_used: result.tools_used.clone(),
            model: Some(self.config.model.clone()),
            token_usage: result.token_usage.clone(),
            cost: result.cost,
        })
    }

    /// Process a message with streaming callback.
    ///
    /// **Legacy method**: uses synchronous callback which cannot .await.
    /// For proper backpressure, use `process_direct_streaming_with_channel` instead.
    pub async fn process_direct_streaming<F>(
        &self,
        content: &str,
        session_key: &SessionKey,
        mut callback: F,
    ) -> Result<AgentResponse, AgentError>
    where
        F: FnMut(stream::StreamEvent) + Send + 'static,
    {
        // For backward compatibility: spawn a task to forward events to callback
        let (mut event_rx, result_handle) = self
            .process_direct_streaming_with_channel(content, session_key)
            .await?;

        // Forward events to callback (this is the old behavior, kept for CLI)
        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                callback(event);
            }
        });

        // Wait for forwarding to complete and get final result
        let (result, _) = tokio::join!(result_handle, forward_handle);
        result.map_err(|e| AgentError::Other(format!("Task join error: {}", e)))?
    }

    /// Process a message with streaming and return a channel for events.
    ///
    /// This is the preferred method for Gateway mode. It returns:
    /// - `mpsc::Receiver<StreamEvent>` - for consuming stream events with .await
    /// - `JoinHandle<Result<AgentResponse>>` - final result after streaming completes
    ///
    /// The caller can now await each event send, providing proper backpressure.
    ///
    /// ## Usage in SessionActor
    pub async fn process_direct_streaming_with_channel(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<stream::StreamEvent>,
            tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
        ),
        AgentError,
    > {
        let session_key_str = session_key.to_string();

        // ── 1. Build initial mutable context for hooks ─────────────
        let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(content)];
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(content),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        // ── 2. BeforeRequest hooks (can modify input or abort) ─────
        match self.hooks.execute(HookPoint::BeforeRequest, &mut ctx).await? {
            HookAction::Abort(msg) => {
                let (_tx, rx) = tokio::sync::mpsc::channel(1);
                let handle = tokio::spawn(async move {
                    Ok(AgentResponse {
                        content: msg,
                        reasoning_content: None,
                        tools_used: vec![],
                        model: Some("error".to_string()),
                        token_usage: None,
                        cost: 0.0,
                    })
                });
                return Ok((rx, handle));
            }
            HookAction::Continue => {}
        }

        // Get the (possibly modified) user content
        let content_str = ctx
            .messages
            .iter()
            .find(|m| m.role == crate::providers::MessageRole::User)
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| content.to_string());

        let session = self.context.load_session(session_key).await;
        let history_snapshot = session.get_history(self.config.memory_window);
        self.context
            .save_message(session_key, "user", &content_str, None)
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

        // ── Assemble prompt ──────────────────────────────────────
        let mut messages = Self::assemble_prompt(
            processed.messages,
            &content_str,
            &system_prompts,
            summary.as_deref(),
            None, // History recall handled by hooks
        );

        // ── AfterHistory and BeforeLLM hooks ─────────────────────
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(&content_str),
            response: None,
            tool_calls: None,
            token_usage: None,
        };
        self.hooks.execute(HookPoint::AfterHistory, &mut ctx).await?;
        self.hooks.execute(HookPoint::BeforeLLM, &mut ctx).await?;

        // Get vault values for log redaction
        let local_vault_values = self.vault_values.read().await.clone();

        // Create channel for stream events
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);

        // Clone data needed for the spawned task
        let provider = self.provider.clone();
        let tools = self.tools.clone();
        let config = self.config.clone();
        let pricing = self.pricing.clone();
        let hooks = self.hooks.clone();
        let context = self.context.clone();
        let session_key_clone = session_key.clone();

        // Spawn task to execute agent loop and handle post-processing
        let result_handle = tokio::spawn(async move {
            use crate::agent::executor_core::{AgentExecutor, ExecutorOptions};

            let executor = AgentExecutor::new(provider, tools, &config);

            let mut options = ExecutorOptions::new().with_vault_values(&local_vault_values);
            if let Some(ref p) = pricing {
                options = options.with_pricing(p.clone());
            }

            // Execute with streaming
            let result = executor
                .execute_stream_with_options(messages, event_tx, &options)
                .await?;

            // ── AfterResponse hooks ───────────────────────────────
            let tools_used: Vec<crate::hooks::ToolCallInfo> = result
                .tools_used
                .iter()
                .map(|name| crate::hooks::ToolCallInfo {
                    id: name.clone(),
                    name: name.clone(),
                    arguments: None,
                })
                .collect();

            let mut ctx = MutableContext {
                session_key: &session_key_str,
                messages: &mut vec![],
                user_input: Some(&content_str),
                response: Some(&result.content),
                tool_calls: Some(&tools_used),
                token_usage: result.token_usage.as_ref(),
            };
            if let Err(e) = hooks.execute(HookPoint::AfterResponse, &mut ctx).await {
                warn!("AfterResponse hook failed (ignored): {}", e);
            }

            // Save to history
            let history_content = redact_secrets(&result.content, &local_vault_values);
            context
                .save_message(
                    &session_key_clone,
                    "assistant",
                    &history_content,
                    Some(result.tools_used.clone()),
                )
                .await;

            // Log token usage if available
            if let Some(ref usage) = result.token_usage {
                info!(
                    "[Token] Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
                    usage.input_tokens, usage.output_tokens, usage.total_tokens, result.cost
                );
            }

            Ok(AgentResponse {
                content: result.content,
                reasoning_content: result.reasoning_content,
                tools_used: result.tools_used,
                model: Some(config.model.clone()),
                token_usage: result.token_usage.clone(),
                cost: result.cost,
            })
        });

        Ok((event_rx, result_handle))
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
        recalled_history: Option<&[String]>,
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

        // 2.5. Inject recalled history (semantic recall of old conversations)
        if let Some(recalled) = recalled_history {
            if !recalled.is_empty() {
                let recall_content = format!(
                    "{}\n{}",
                    crate::agent::summarization::RECALL_PREFIX,
                    recalled.join("\n")
                );
                messages.push(ChatMessage::assistant(recall_content));
                debug!("Injected {} recalled history messages", recalled.len());
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
            "Built messages: {} history msgs, summary: {}, recalled: {}",
            history_count,
            summary.is_some(),
            recalled_history.map(|r| r.len()).unwrap_or(0)
        );

        messages
    }
}
