//! Agent loop: the core processing engine
//!
//! ## Execution Flow
//!
//! The main pipeline in 'process_direct_with_callback' is a straight-line sequence:
//!
//! 1. external_hook(pre_request)  → shell script can abort or modify input
//! 2. load_session                → inline: Option<SessionManager> loads history
//! 3. save_user_message           → inline: Option<SessionManager> persists user msg
//! 4. process_history             → pure: truncate history, compute evictions
//! 5. load_summary + bg_compress  → load existing summary (fast), spawn background compression if messages were evicted (non-blocking)
//! 6. inject_system_prompts       → direct: bootstrap + skills
//! 7. assemble_prompt             → pure: build Vec<ChatMessage>
//!    7.5. run_interceptors          → middleware chain (vault injection, etc.)
//! 8. run_agent_loop              → LLM iteration (with inline logging)
//! 9. external_hook(post_response) → shell script for audit/alerting
//! 10. save_assistant_msg          → inline: Option<SessionManager> persists assistant msg
//!
//! All steps are **direct method calls** or pure functions — no hidden hook dispatch.
//! External shell hooks (if present) are called via subprocess at steps 1 and 10.
//! Step 5's background compression uses `tokio::spawn` — zero user-facing latency.
//! Step 7.5 uses the interceptor chain for pluggable message transformation.
//!
//! ## Option<T> vs Trait Objects
//!
//! The agent uses 'Option<T>' for storage dependencies instead of trait objects.
//! Main agents get 'Some(real_implementation)'; subagents get 'None'.
//! This is more explicit and avoids virtual dispatch overhead in hot paths.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::executor::ToolExecutor;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::interceptor::InterceptorChain;
use crate::agent::prompt;
use crate::agent::request::RequestHandler;
use crate::agent::stream::{self, StreamCallback, StreamEvent};
use crate::bus::events::SessionKey;
use crate::error::AgentError;
use crate::hooks::ExternalHookRunner;
use crate::providers::{ChatMessage, ChatResponse, LlmProvider};
use crate::tools::ToolRegistry;
use crate::vault::{redact_secrets, VaultInjector, VaultStore};

use crate::agent::memory::MemoryStore;
use crate::agent::summarization::{ContextCompressionHook, SummarizationService};
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

/// Mutable state for tool call handling.
struct ToolCallState<'a> {
    /// Conversation messages
    messages: &'a mut Vec<ChatMessage>,
    /// List of tools used so far
    tools_used: &'a mut Vec<String>,
}

/// Model pricing configuration for cost calculation
#[derive(Debug, Clone)]
struct ModelPricingInfo {
    /// Price per million input tokens
    price_input_per_million: f64,
    /// Price per million output tokens
    price_output_per_million: f64,
    /// Currency code
    currency: String,
}

// ── Inline logging functions (replaces LoggingHook) ─────────

/// Log an LLM response — reasoning and content.
/// Redacts sensitive values if provided.
fn log_llm_response(response: &ChatResponse, iteration: u32, vault_values: &[String]) {
    if let Some(ref reasoning) = response.reasoning_content {
        if !reasoning.is_empty() {
            let safe_reasoning = redact_secrets(reasoning, vault_values);
            debug!("[Agent] Reasoning (iter {}): {}", iteration, safe_reasoning);
        }
    }
    if let Some(ref content) = response.content {
        if !content.is_empty() {
            let safe_content = redact_secrets(content, vault_values);
            info!("[Agent] Response (iter {}): {}", iteration, safe_content);
        }
    }
}

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// Uses **Option<T>** for storage dependencies (explicit null handling):
/// - **SessionManager** — session persistence (load/save messages)
/// - **MemoryStore** — provides SqliteStore for session/summary management
/// - **SummarizationService** — LLM-based context compression
///
/// Main agents get 'Some(real_implementation)'; subagents get 'None'.
/// This is more explicit than trait objects and avoids virtual dispatch.
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
/// via the `interceptors` chain — pluggable middleware pattern.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
    /// History truncator configuration.
    history_config: HistoryConfig,
    /// Session persistence — 'None' for subagents.
    session_manager: Option<Arc<SessionManager>>,
    /// Context compression service — 'None' for subagents.
    summarization: Option<Arc<SummarizationService>>,
    /// Pre-loaded system prompt (from workspace bootstrap files).
    system_prompt: String,
    /// Pre-loaded skills context (from workspace skills).
    skills_context: Option<String>,
    /// External shell hook runner (pre_request / post_response).
    external_hooks: ExternalHookRunner,
    /// Message interceptor chain for pre-LLM processing (vault injection, etc.)
    interceptors: InterceptorChain,
    /// Injected values for log redaction (thread-safe).
    vault_values: Mutex<Vec<String>>,
    /// Token usage tracker for the session
    session_stats: Mutex<crate::token_tracker::SessionTokenStats>,
    /// Model pricing configuration (optional)
    pricing: Option<ModelPricingInfo>,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Owns core services via 'Option<T>' (explicit null handling):
    /// - **SessionManager** — session load/save
    /// - **MemoryStore** — provides SqliteStore for sessions/summaries
    /// - **SummarizationService** — context compression
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
        tools: ToolRegistry,
    ) -> Result<Self, AgentError> {
        let memory_store = Arc::new(MemoryStore::new().await);
        let session_manager = Arc::new(SessionManager::new(memory_store.sqlite_store().clone()));

        let store_arc = memory_store.sqlite_store().clone();
        let summarization = Arc::new(SummarizationService::new(
            provider.clone(),
            Arc::new(store_arc),
            config.model.clone(),
        ));

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

        // Initialize interceptor chain with vault injector (optional - for sensitive data isolation)
        let mut interceptors = InterceptorChain::new();
        if let Ok(store) = VaultStore::new() {
            debug!("[Agent] Vault initialized successfully, adding vault interceptor");
            interceptors.add(Box::new(VaultInjector::new(Arc::new(store))));
        } else {
            debug!("[Agent] Vault not available, skipping vault interceptor");
        }

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            session_manager: Some(session_manager),
            summarization: Some(summarization),
            system_prompt,
            skills_context,
            external_hooks,
            interceptors,
            vault_values: Mutex::new(Vec::new()),
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new("USD")),
            pricing: None,
        })
    }

    /// Create a new agent loop with an **externally created** 'MemoryStore'.
    ///
    /// Use this when the 'MemoryStore' must be shared with the session manager
    /// and summarization service.
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
        let session_manager = Arc::new(SessionManager::new(memory_store.sqlite_store().clone()));

        let store_arc = memory_store.sqlite_store().clone();
        let summarization = Arc::new(SummarizationService::new(
            provider.clone(),
            Arc::new(store_arc),
            config.model.clone(),
        ));

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

        // Initialize interceptor chain with vault injector (optional - for sensitive data isolation)
        let mut interceptors = InterceptorChain::new();
        if let Ok(store) = VaultStore::new() {
            debug!("[Agent] Vault initialized successfully, adding vault interceptor");
            interceptors.add(Box::new(VaultInjector::new(Arc::new(store))));
        } else {
            debug!("[Agent] Vault not available, skipping vault interceptor");
        }

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            session_manager: Some(session_manager),
            summarization: Some(summarization),
            system_prompt,
            skills_context,
            external_hooks,
            interceptors,
            vault_values: Mutex::new(Vec::new()),
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new("USD")),
            pricing: None,
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// System prompt is empty by default; use 'set_system_prompt()' to configure.
    /// Subagents get 'None' for all storage services (no persistence).
    /// No external hooks for subagents.
    /// No vault for subagents (empty interceptor chain).
    pub fn builder(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            session_manager: None,
            summarization: None,
            system_prompt: String::new(),
            skills_context: None,
            external_hooks: ExternalHookRunner::noop(),
            interceptors: InterceptorChain::new(), // Empty chain for subagents
            vault_values: Mutex::new(Vec::new()),
            session_stats: Mutex::new(crate::token_tracker::SessionTokenStats::new("USD")),
            pricing: None,
        })
    }

    /// Set the system prompt (used by subagents to configure identity).
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Set the pricing configuration for cost calculation.
    pub fn set_pricing(
        &mut self,
        price_input_per_million: f64,
        price_output_per_million: f64,
        currency: &str,
    ) {
        self.pricing = Some(ModelPricingInfo {
            price_input_per_million,
            price_output_per_million,
            currency: currency.to_string(),
        });
        // Also update session stats currency
        if let Ok(mut stats) = self.session_stats.lock() {
            stats.currency = currency.to_string();
        }
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
    /// No-op for subagents (no session storage).
    pub async fn clear_session(&self, session_key: &SessionKey) {
        if let Some(ref sm) = self.session_manager {
            sm.clear_session(session_key).await;
        }
    }

    /// Process a message directly (for CLI or testing)
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        self.process_direct_with_callback(content, session_key, None)
            .await
    }

    /// Process a message with optional streaming callback.
    ///
    /// The pipeline is a straight-line sequence with no hidden control flow.
    /// See module-level docs for the full execution flow diagram.
    pub async fn process_direct_with_callback(
        &self,
        content: &str,
        session_key: &SessionKey,
        callback: Option<&StreamCallback>,
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

        // ── 2. Load session history (direct, no trait indirection) ─────
        let session = match &self.session_manager {
            Some(sm) => sm.get_or_create(session_key).await,
            None => crate::session::Session::from_key(session_key.clone()),
        };
        let history_snapshot = session.get_history(self.config.memory_window);

        // ── 3. Save user message (direct, Option-aware) ────────────────
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.append_by_key(session_key, "user", content, None).await {
                warn!("Failed to persist user message: {}", e);
            }
        }

        // ── 4. Truncate history (pure computation) ─────────────────
        let processed = process_history(history_snapshot, &self.history_config);

        // ── 5. Load existing summary + spawn background compression ─────
        //
        // The summarization LLM call is expensive (~10-30s). Instead of blocking
        // the user's response, we:
        //   a) Load the existing (possibly stale) summary — cheap SQLite read.
        //   b) If there are evictions, fire off a background `tokio::spawn` task
        //      to generate an updated summary. It will be available next turn.
        //   c) Use the existing summary for this turn's prompt assembly.
        let summary = match &self.summarization {
            Some(s) => {
                // Always load the existing summary (fast, no LLM call)
                let existing = s.load_summary(&session_key_str).await;

                // If messages were evicted, spawn background compression
                if !processed.evicted.is_empty() {
                    let svc = Arc::clone(s);
                    let key = session_key_str.clone();
                    let evicted = processed.evicted.clone();

                    tokio::spawn(async move {
                        debug!(
                            "[Summarization] Background compression task started for session '{}'",
                            key
                        );
                        match svc.compress(&key, &evicted).await {
                            Ok(_) => {
                                debug!(
                                    "[Summarization] Background compression completed for session '{}'",
                                    key
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "[Summarization] Background compression failed for session '{}': {}",
                                    key, e
                                );
                            }
                        }
                    });
                }

                existing
            }
            None => None,
        };

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

        // ── 7.5. Run interceptor chain (vault injection, etc.) ────────
        // Interceptors modify messages in-place before sending to LLM.
        // The vault injector is one such interceptor; others can be added.
        let _interceptor_reports = self.interceptors.run(&mut messages);

        // Note: vault_values are now managed internally by the vault interceptor.
        // For log redaction, we need to track what was injected.
        // This is a simplified approach - in production, interceptors would report injected values.

        // ── 8. Run agent loop ─────────────────────────────────────
        // For WebSocket and other real-time channels, always use the callback if provided,
        // regardless of the streaming config. The streaming config is for CLI output formatting,
        // but WebSocket clients expect real-time events.
        let effective_cb = if callback.is_some() {
            // If a callback was explicitly provided (e.g., for WebSocket), use it
            callback
        } else if self.config.streaming {
            // Otherwise, respect the streaming config
            callback
        } else {
            None
        };
        let result = self.run_agent_loop(messages, effective_cb).await?;

        // ── 9. External hook: post_response (audit / alerting) ────
        let tools_used_str = result.tools_used.join(", ");

        // Redact secrets from post_response hook
        let safe_content = if let Ok(values) = self.vault_values.lock().as_ref() {
            redact_secrets(&result.content, values)
        } else {
            result.content.clone()
        };

        if let Err(e) = self
            .external_hooks
            .run_post_response(&session_key_str, &safe_content, &tools_used_str)
            .await
        {
            warn!("post_response hook failed (ignored): {}", e);
        }

        // ── 10. Save assistant message (direct, Option-aware) ──────────
        // Redact secrets before saving to history
        if let Some(ref sm) = self.session_manager {
            let history_content = if let Ok(values) = self.vault_values.lock().as_ref() {
                redact_secrets(&result.content, values)
            } else {
                result.content.clone()
            };

            if let Err(e) = sm
                .append_by_key(
                    session_key,
                    "assistant",
                    &history_content,
                    Some(result.tools_used.clone()),
                )
                .await
            {
                warn!("Failed to persist assistant message: {}", e);
            }
        }

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
        })
    }

    // ── Agent Loop Internals ────────────────────────────────

    /// Calculate token usage and cost for a response.
    ///
    /// Uses API-provided usage when available, falls back to tiktoken-rs estimation.
    fn calculate_token_usage(
        &self,
        response: &ChatResponse,
        _model: &str,
    ) -> (Option<crate::token_tracker::TokenUsage>, f64) {
        // Try to get usage from API response first
        if let Some(usage) = &response.usage {
            let token_usage = crate::token_tracker::TokenUsage::from_api_fields(
                usage.input_tokens,
                usage.output_tokens,
            );
            // Calculate cost if pricing is configured
            let cost = if let Some(ref pricing) = self.pricing {
                let input_cost =
                    (usage.input_tokens as f64) * pricing.price_input_per_million / 1_000_000.0;
                let output_cost =
                    (usage.output_tokens as f64) * pricing.price_output_per_million / 1_000_000.0;
                input_cost + output_cost
            } else {
                0.0
            };
            return (Some(token_usage), cost);
        }

        // Fallback: estimate tokens using tiktoken-rs
        let mut output_tokens = 0;

        // Estimate output from response content
        if let Some(ref content) = response.content {
            output_tokens = crate::token_tracker::estimate_tokens(content);
        }

        // We can't accurately estimate input tokens without access to the full request
        // For now, use a rough heuristic based on output tokens
        let input_tokens = output_tokens * 2; // Rough estimate: input is typically 2x output

        let token_usage = crate::token_tracker::TokenUsage::new(input_tokens, output_tokens);

        // Calculate cost for estimated tokens if pricing is configured
        let cost = if let Some(ref pricing) = self.pricing {
            let input_cost = (input_tokens as f64) * pricing.price_input_per_million / 1_000_000.0;
            let output_cost =
                (output_tokens as f64) * pricing.price_output_per_million / 1_000_000.0;
            input_cost + output_cost
        } else {
            0.0
        };

        (Some(token_usage), cost)
    }

    /// Unified agent iteration loop.
    ///
    /// Always uses 'chat_stream' — for non-streaming providers the default
    /// trait impl wraps the response in a single-chunk stream, so both paths
    /// converge here.  When 'callback' is 'None', stream events are silently
    /// discarded.
    async fn run_agent_loop(
        &self,
        initial_messages: Vec<ChatMessage>,
        callback: Option<&StreamCallback>,
    ) -> Result<AgentLoopResult, AgentError> {
        let noop: StreamCallback = Box::new(|_| {});
        let cb: &StreamCallback = callback.unwrap_or(&noop);

        // Get vault values for log redaction
        let vault_values: Vec<String> = self
            .vault_values
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default();

        let mut messages = initial_messages;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, &self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("Agent loop iteration {}", iteration);

            let request = request_handler.build_chat_request(&messages);
            let model = request.model.clone();
            let mut stream_result = request_handler.send_with_retry(request).await?;
            let response = stream::accumulate_stream(&mut stream_result, cb).await?;

            // Calculate token usage and cost
            let (token_usage, cost) = self.calculate_token_usage(&response, &model);

            // Log token/cost info
            if let Some(ref usage) = token_usage {
                let currency = self
                    .pricing
                    .as_ref()
                    .map(|p| p.currency.as_str())
                    .unwrap_or("USD");
                let pricing_ref = self.pricing.as_ref().map(|p| {
                    crate::token_tracker::ModelPricing::new(
                        p.price_input_per_million,
                        p.price_output_per_million,
                        &p.currency,
                    )
                });
                info!(
                    "[Token] {}",
                    crate::token_tracker::format_request_stats(
                        usage,
                        cost,
                        currency,
                        pricing_ref.as_ref()
                    )
                );
            }

            // Inline logging (replaces LoggingHook::on_llm_response)
            log_llm_response(&response, iteration, &vault_values);

            let has_tools = response.has_tool_calls();
            info!(
                "[Agent] iter {} has_tool_calls={}, tool_count={}",
                iteration,
                has_tools,
                response.tool_calls.len()
            );

            if !has_tools {
                info!("[Agent] No tool calls, sending Done event and returning response");
                let content = response.content.unwrap_or_else(|| {
                    "I've completed processing but have no response to give.".to_string()
                });
                cb(&StreamEvent::Done);

                // Track session stats
                if let Some(ref usage) = token_usage {
                    if let Ok(mut stats) = self.session_stats.lock() {
                        stats.add_usage(usage, cost);
                    }
                }

                return Ok(AgentLoopResult {
                    content,
                    reasoning_content: response.reasoning_content,
                    tools_used,
                    token_usage,
                    cost,
                });
            }

            // Has tool calls — execute them and continue the loop
            let mut state = ToolCallState {
                messages: &mut messages,
                tools_used: &mut tools_used,
            };
            self.handle_tool_calls(&response, &executor, &mut state, cb)
                .await;
        }

        // Exhausted max iterations without a final response
        // Send Done event to signal stream completion
        cb(&StreamEvent::Done);

        Ok(AgentLoopResult {
            content: "I've completed processing but have no response to give.".to_string(),
            reasoning_content: None,
            tools_used,
            token_usage: None,
            cost: 0.0,
        })
    }

    /// Execute tool calls, append results to messages, and update tracking.
    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        state: &mut ToolCallState<'_>,
        cb: &StreamCallback,
    ) {
        info!(
            "[Agent] Executing {} tool call(s): {}",
            response.tool_calls.len(),
            response
                .tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Add assistant message with tool calls to the conversation
        if response.tool_calls.is_empty() {
            if let Some(ref c) = response.content {
                state.messages.push(ChatMessage::assistant(c));
            }
        } else {
            state.messages.push(ChatMessage::assistant_with_tools(
                response.content.clone(),
                response.tool_calls.clone(),
            ));
        }

        // Emit ToolStart events before execution
        for tool_call in &response.tool_calls {
            cb(&StreamEvent::ToolStart {
                name: tool_call.function.name.clone(),
                arguments: Some(tool_call.function.arguments.to_string()),
            });
        }

        // Execute tool calls sequentially to prevent race conditions
        // and maintain LLM's expected causal ordering
        for tool_call in &response.tool_calls {
            let start = std::time::Instant::now();
            let result = executor.execute_one(tool_call).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let tool_name = tool_call.function.name.clone();

            // Inline logging (replaces LoggingHook::on_tool_result)
            debug!("[Tool] {} -> done ({}ms)", tool_name, duration_ms);

            state.tools_used.push(tool_name.clone());

            cb(&StreamEvent::ToolEnd {
                name: tool_name.clone(),
                output: result.output.clone(),
            });
            // Add the tool result to the conversation
            state.messages.push(ChatMessage::tool_result(
                tool_call.id.clone(),
                tool_name.clone(),
                result.output,
            ));
        }
    }
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
