//! Agent loop: the core processing engine
//!
//! ## Execution Flow
//!
//! The main pipeline in `process_direct_with_callback` is a straight-line sequence:
//!
//! ```text
//! 1. external_hook(pre_request)  → shell script can abort or modify input
//! 2. load_session                → inline: Option<SessionManager> loads history
//! 3. save_user_message           → inline: Option<SessionManager> persists user msg
//! 4. process_history             → pure: truncate history, compute evictions
//! 5. summarize                   → inline: Option<SummarizationService> compresses evicted msgs
//! 6. inject_system_prompts       → direct: bootstrap + skills
//! 7. read_long_term              → inline: Option<MemoryStore> reads MEMORY.md
//! 8. assemble_prompt             → pure: build Vec<ChatMessage>
//! 9. run_agent_loop              → LLM iteration (with inline logging)
//! 10. external_hook(post_response) → shell script for audit/alerting
//! 11. save_assistant_msg          → inline: Option<SessionManager> persists assistant msg
//! ```
//!
//! All steps are **direct method calls** or pure functions — no hidden hook dispatch.
//! External shell hooks (if present) are called via subprocess at steps 1 and 10.
//!
//! ## Option<T> vs Trait Objects
//!
//! The agent uses `Option<T>` for storage dependencies instead of trait objects.
//! Main agents get `Some(real_implementation)`; subagents get `None`.
//! This is more explicit and avoids virtual dispatch overhead in hot paths.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::executor::ToolExecutor;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::prompt;
use crate::agent::request::RequestHandler;
use crate::agent::stream::{self, StreamCallback, StreamEvent};
use crate::bus::events::SessionKey;
use crate::error::AgentError;
use crate::hooks::ExternalHookRunner;
use crate::providers::{ChatMessage, ChatResponse, LlmProvider};
use crate::tools::ToolRegistry;

use crate::agent::memory::MemoryStore;
use crate::agent::summarization::{ContextCompressionHook, SummarizationService};
use crate::session::SessionManager;

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
            model: "gpt-4o".to_string(),
            max_iterations: 20,
            temperature: 1.0,
            max_tokens: 65536,
            memory_window: 50,
            max_tool_result_chars: 8000,
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
struct AgentLoopResult {
    /// Main response content
    content: String,
    /// Reasoning/thinking content (if thinking mode enabled)
    reasoning_content: Option<String>,
    /// Tools used during processing
    tools_used: Vec<String>,
}

/// Mutable state for tool call handling.
struct ToolCallState<'a> {
    /// Conversation messages
    messages: &'a mut Vec<ChatMessage>,
    /// List of tools used so far
    tools_used: &'a mut Vec<String>,
}

// ── Inline logging functions (replaces LoggingHook) ─────────

/// Log an LLM response — reasoning and content.
fn log_llm_response(response: &ChatResponse, iteration: u32) {
    if let Some(ref reasoning) = response.reasoning_content {
        if !reasoning.is_empty() {
            debug!("[Agent] Reasoning (iter {}): {}", iteration, reasoning);
        }
    }
    if let Some(ref content) = response.content {
        if !content.is_empty() {
            info!("[Agent] Response (iter {}): {}", iteration, content);
        }
    }
}

/// Log a tool result — name, preview, and duration.
fn log_tool_result(tool_name: &str, tool_result: &str, duration_ms: u64) {
    let preview = if tool_result.len() > 500 {
        format!("{}... (truncated)", &tool_result[..500])
    } else {
        tool_result.to_string()
    };
    debug!("[Tool] {} -> {} ({}ms)", tool_name, preview, duration_ms);
}

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// Uses **Option<T>** for storage dependencies (explicit null handling):
/// - **SessionManager** — session persistence (load/save messages)
/// - **MemoryStore** — long-term memory (MEMORY.md)
/// - **SummarizationService** — LLM-based context compression
///
/// Main agents get `Some(real_implementation)`; subagents get `None`.
/// This is more explicit than trait objects and avoids virtual dispatch.
///
/// System prompt and skills context are loaded **once** at initialization
/// and stored as plain `String` fields — no dynamic dispatch.
///
/// External shell hooks (`~/.nanobot/hooks/`) are invoked at request
/// boundaries (pre_request / post_response) via subprocess — UNIX philosophy.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
    /// History truncator configuration.
    history_config: HistoryConfig,
    /// Session persistence — `None` for subagents.
    session_manager: Option<Arc<SessionManager>>,
    /// Long-term memory store — `None` for subagents.
    memory_store: Option<Arc<MemoryStore>>,
    /// Context compression service — `None` for subagents.
    summarization: Option<Arc<SummarizationService>>,
    /// Pre-loaded system prompt (from workspace bootstrap files).
    system_prompt: String,
    /// Pre-loaded skills context (from workspace skills).
    skills_context: Option<String>,
    /// External shell hook runner (pre_request / post_response).
    external_hooks: ExternalHookRunner,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Owns core services via `Option<T>` (explicit null handling):
    /// - **SessionManager** — session load/save
    /// - **MemoryStore** — long-term memory
    /// - **SummarizationService** — context compression
    ///
    /// Loads system prompt and skills context **once** at initialization.
    /// Logging is inlined directly — no hook indirection.
    /// External shell hooks are loaded from `~/.nanobot/hooks/`.
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

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            session_manager: Some(session_manager),
            memory_store: Some(memory_store),
            summarization: Some(summarization),
            system_prompt,
            skills_context,
            external_hooks,
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// System prompt is empty by default; use `set_system_prompt()` to configure.
    /// Subagents get `None` for all storage services (no persistence).
    /// No external hooks for subagents.
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
            memory_store: None,
            summarization: None,
            system_prompt: String::new(),
            skills_context: None,
            external_hooks: ExternalHookRunner::noop(),
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

    /// Clear the session for the given key (used by CLI for `/new` command).
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

        // ── 5. Summarize evicted messages (direct, Option-aware) ───────
        let summary = match &self.summarization {
            Some(s) if !processed.evicted.is_empty() => {
                match s.compress(&session_key_str, &processed.evicted).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Summarization failed: {}", e);
                        None
                    }
                }
            }
            Some(s) => {
                // No evictions — still try to load existing summary
                s.compress(&session_key_str, &[]).await.unwrap_or_default()
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

        // ── 7. Read long-term memory (direct, Option-aware) ────────────
        let memory_content = match &self.memory_store {
            Some(ms) => match ms.read_long_term().await {
                Ok(mem) if !mem.is_empty() => Some(mem),
                Ok(_) => None,
                Err(e) => {
                    warn!("Failed to read long-term memory: {}", e);
                    None
                }
            },
            None => None,
        };

        // ── 8. Assemble prompt (pure, synchronous) ─────────────────
        let messages = Self::assemble_prompt(
            processed.messages,
            content,
            &system_prompts,
            memory_content.as_deref(),
            summary.as_deref(),
        );

        // ── 9. Run agent loop ─────────────────────────────────────
        let effective_cb = if self.config.streaming {
            callback
        } else {
            None
        };
        let result = self.run_agent_loop(messages, effective_cb).await?;

        // ── 10. External hook: post_response (audit / alerting) ────
        let tools_used_str = result.tools_used.join(", ");
        if let Err(e) = self
            .external_hooks
            .run_post_response(&session_key_str, &result.content, &tools_used_str)
            .await
        {
            warn!("post_response hook failed (ignored): {}", e);
        }

        // ── 11. Save assistant message (direct, Option-aware) ──────────
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm
                .append_by_key(
                    session_key,
                    "assistant",
                    &result.content,
                    Some(result.tools_used.clone()),
                )
                .await
            {
                warn!("Failed to persist assistant message: {}", e);
            }
        }

        Ok(AgentResponse {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
        })
    }

    // ── Agent Loop Internals ────────────────────────────────

    /// Unified agent iteration loop.
    ///
    /// Always uses `chat_stream` — for non-streaming providers the default
    /// trait impl wraps the response in a single-chunk stream, so both paths
    /// converge here.  When `callback` is `None`, stream events are silently
    /// discarded.
    async fn run_agent_loop(
        &self,
        initial_messages: Vec<ChatMessage>,
        callback: Option<&StreamCallback>,
    ) -> Result<AgentLoopResult, AgentError> {
        let noop: StreamCallback = Box::new(|_| {});
        let cb: &StreamCallback = callback.unwrap_or(&noop);

        let mut messages = initial_messages;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, &self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("Agent loop iteration {}", iteration);

            let request = request_handler.build_chat_request(&messages);
            let mut stream_result = request_handler.send_with_retry(request).await?;
            let response = stream::accumulate_stream(&mut stream_result, cb).await?;

            // Inline logging (replaces LoggingHook::on_llm_response)
            log_llm_response(&response, iteration);

            if !response.has_tool_calls() {
                let content = response.content.unwrap_or_else(|| {
                    "I've completed processing but have no response to give.".to_string()
                });
                cb(&StreamEvent::Done);
                return Ok(AgentLoopResult {
                    content,
                    reasoning_content: response.reasoning_content,
                    tools_used,
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
        Ok(AgentLoopResult {
            content: "I've completed processing but have no response to give.".to_string(),
            reasoning_content: None,
            tools_used,
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
            log_tool_result(&tool_name, &result.output, duration_ms);

            state.tools_used.push(tool_name.clone());

            let output_preview = truncate_preview(&result.output, 500);

            cb(&StreamEvent::ToolEnd {
                name: tool_name.clone(),
                output: output_preview,
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
        memory: Option<&str>,
        summary: Option<&str>,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // 1. Build the system prompt
        let mut system_content = system_prompts.join("\n\n");
        if let Some(mem) = memory {
            if !mem.is_empty() {
                system_content.push_str("\n\n## Long-term Memory\n");
                system_content.push_str(mem);
            }
        }
        if !system_content.is_empty() {
            messages.push(ChatMessage::system(system_content));
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

        // 3. Add processed history messages
        let history_count = processed_history.len();
        for msg in processed_history {
            match msg.role {
                crate::providers::MessageRole::User => {
                    messages.push(ChatMessage::user(&msg.content))
                }
                crate::providers::MessageRole::Assistant => {
                    messages.push(ChatMessage::assistant(&msg.content))
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

/// Truncate a string for preview logging, respecting UTF-8 char boundaries.
fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i < max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}... (truncated, {} chars total)", &s[..end], s.len())
}
