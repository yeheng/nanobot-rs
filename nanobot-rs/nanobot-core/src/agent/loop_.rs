//! Agent loop: the core processing engine
//!
//! ## Execution Flow
//!
//! The main pipeline in `process_direct_with_callback` is a straight-line sequence:
//!
//! ```text
//! 1. on_request         → hooks can skip the request
//! 2. load_session       → inline: SessionStorage loads history
//! 3. save_user_message  → inline: SessionStorage persists user msg
//! 4. process_history    → pure: truncate history, compute evictions
//! 5. summarize          → inline: ContextCompressionHook compresses evicted msgs
//! 6. on_context_prepare → hooks inject system_prompts (bootstrap, skills)
//! 7. read_long_term     → inline: LongTermMemory reads MEMORY.md
//! 8. assemble_prompt    → pure: build Vec<ChatMessage>
//! 9. run_agent_loop     → LLM iteration (with on_llm_request/response, tool hooks)
//! 10. on_response       → hooks post-process
//! 11. save_assistant_msg → inline: SessionStorage persists assistant msg
//! ```
//!
//! Steps 2, 3, 5, 7, 11 are **direct method calls** on trait objects
//! (not hidden behind hooks), making the data flow explicit and debuggable.
//!
//! ## Null Object Pattern
//!
//! The agent uses trait objects (`Arc<dyn SessionStorage>`, etc.) instead of
//! `Option<T>`. Subagents get no-op implementations, eliminating `if let Some`
//! checks throughout the code. This is cleaner than special-case control flow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, instrument, warn};

use crate::agent::executor::ToolExecutor;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::request::RequestHandler;
use crate::agent::storage::{LongTermMemory, SessionStorage};
use crate::agent::stream::{self, StreamCallback, StreamEvent};
use crate::agent::summarization::ContextCompressionHook;
use crate::error::AgentError;
use crate::hooks::logging::LoggingHook;
use crate::hooks::prompt;
use crate::hooks::{
    AgentHook, ContextPrepareContext, HookRegistry, LlmRequestContext, LlmResponseContext,
    RequestContext, ResponseContext, ToolExecuteContext, ToolResultContext,
};
use crate::providers::{ChatMessage, ChatResponse, LlmProvider};
use crate::tools::ToolRegistry;

use crate::agent::memory::MemoryStore;
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
    /// Metadata collected from hooks
    metadata: HashMap<String, serde_json::Value>,
}

/// Mutable state for tool call handling.
struct ToolCallState<'a> {
    /// Conversation messages
    messages: &'a mut Vec<ChatMessage>,
    /// List of tools used so far
    tools_used: &'a mut Vec<String>,
    /// Metadata collected from hooks
    hook_metadata: &'a mut HashMap<String, serde_json::Value>,
}

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// Uses **trait objects** for storage dependencies (Null Object pattern):
/// - **SessionStorage** — session persistence (load/save messages)
/// - **LongTermMemory** — long-term memory (MEMORY.md)
/// - **ContextCompressionHook** — LLM-based context compression
///
/// Main agents get real implementations; subagents get no-op implementations.
/// This eliminates `Option<T>` checks throughout the code.
///
/// System prompt and skills context are loaded **once** at initialization
/// and stored as plain `String` fields — no dynamic hook dispatch.
///
/// Uses **HookRegistry** only for truly extensible concerns:
/// - Logging (LLM response + tool result logging)
/// - Custom extensions via `register_hook()`
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
    /// Lifecycle hooks for extensible (non-core) behavior.
    hooks: HookRegistry,
    /// History truncator configuration.
    history_config: HistoryConfig,
    /// Session persistence — trait object for Null Object pattern.
    sessions: Arc<dyn SessionStorage>,
    /// Long-term memory store — trait object for Null Object pattern.
    memory: Arc<dyn LongTermMemory>,
    /// Context compression — trait object for Null Object pattern.
    compression: Arc<dyn ContextCompressionHook>,
    /// Pre-loaded system prompt (from workspace bootstrap files).
    /// Injected directly in step 6 — no hook dispatch.
    system_prompt: String,
    /// Pre-loaded skills context (from workspace skills).
    /// Injected directly in step 6 — no hook dispatch.
    skills_context: Option<String>,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Owns core services via trait objects (Null Object pattern):
    /// - **SessionStorage** — session load/save
    /// - **LongTermMemory** — long-term memory
    /// - **ContextCompressionHook** — context compression
    ///
    /// Loads system prompt and skills context **once** at initialization
    /// and injects them directly into the prompt — no hook dispatch.
    ///
    /// Registers hooks only for truly extensible concerns:
    /// - **LoggingHook** — LLM + tool logging
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
        let memory = MemoryStore::new().await;
        let sessions = SessionManager::new(memory.sqlite_store().clone());

        let store_arc = Arc::new(memory.sqlite_store().clone());
        let compression = Arc::new(crate::agent::summarization::SummarizationService::new(
            provider.clone(),
            store_arc,
            config.model.clone(),
        ));

        // Load system prompt and skills directly — no hook indirection
        let system_prompt =
            prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(&workspace).await;

        // Only register hooks for truly extensible concerns
        let mut hooks = HookRegistry::new();
        hooks.register(Arc::new(LoggingHook::new()));

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            hooks,
            history_config: HistoryConfig::default(),
            sessions: Arc::new(crate::agent::storage::RealSessionStorage(sessions)),
            memory: Arc::new(crate::agent::storage::RealLongTermMemory(memory)),
            compression,
            system_prompt,
            skills_context,
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// System prompt is empty by default; use `set_system_prompt()` to configure.
    /// Subagents get no-op storage implementations (Null Object pattern).
    pub async fn builder(
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
            hooks: HookRegistry::new(),
            history_config: HistoryConfig {
                max_messages: 20,
                token_budget: 4000,
                recent_keep: 5,
            },
            sessions: Arc::new(crate::agent::storage::NoopSessionStorage),
            memory: Arc::new(crate::agent::storage::NoopLongTermMemory),
            compression: Arc::new(crate::agent::storage::NoopContextCompression),
            system_prompt: String::new(),
            skills_context: None,
        })
    }

    /// Set the system prompt (used by subagents to configure identity).
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Register a lifecycle hook.
    ///
    /// Hooks are invoked in registration order at each lifecycle stage.
    pub fn register_hook(&mut self, hook: Arc<dyn AgentHook>) {
        self.hooks.register(hook);
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
    pub async fn clear_session(&self, session_key: &str) {
        self.sessions.clear_session(session_key).await;
    }

    /// Process a message directly (for CLI or testing)
    #[instrument(skip(self, content))]
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &str,
    ) -> Result<AgentResponse, AgentError> {
        self.process_direct_with_callback(content, session_key, None)
            .await
    }

    /// Process a message with optional streaming callback.
    ///
    /// The pipeline is a straight-line sequence with no hidden control flow.
    /// See module-level docs for the full execution flow diagram.
    #[instrument(skip(self, content, callback))]
    pub async fn process_direct_with_callback(
        &self,
        content: &str,
        session_key: &str,
        callback: Option<&StreamCallback>,
    ) -> Result<AgentResponse, AgentError> {
        let request_id = uuid::Uuid::new_v4().to_string();

        // ── 1. Hook: on_request (can skip) ────────────────────────
        let mut req_ctx = RequestContext {
            request_id: request_id.clone(),
            session_key: session_key.to_string(),
            user_message: content.to_string(),
            skip: false,
            metadata: HashMap::new(),
        };
        self.hooks.run_on_request(&mut req_ctx).await;
        if req_ctx.skip {
            return Ok(AgentResponse {
                content: String::new(),
                reasoning_content: None,
                tools_used: vec![],
            });
        }
        let mut metadata = req_ctx.metadata;

        // ── 2. Load session history (direct via trait) ─────────────
        let session = self.sessions.get_or_create(session_key).await;
        let history_snapshot = session.get_history(self.config.memory_window);

        // ── 3. Save user message (direct via trait) ────────────────
        if let Err(e) = self
            .sessions
            .append_by_key(session_key, "user", content, None)
            .await
        {
            warn!("Failed to persist user message: {}", e);
        }

        // ── 4. Truncate history (pure computation) ─────────────────
        let processed = process_history(history_snapshot, &self.history_config);

        // ── 5. Summarize evicted messages (direct via trait) ───────
        let summary = if !processed.evicted.is_empty() {
            match self
                .compression
                .compress(session_key, &processed.evicted)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    warn!("Summarization failed: {}", e);
                    None
                }
            }
        } else {
            // No evictions — still try to load existing summary
            self.compression
                .compress(session_key, &[])
                .await
                .unwrap_or_default()
        };

        // ── 6. Inject system prompts (direct) ──────────────────────
        //    Previously done via BootstrapHook/SkillsHook dynamic dispatch;
        //    now inlined for clarity and zero overhead.
        let mut ctx_prepare = ContextPrepareContext {
            request_id: request_id.clone(),
            session_key: session_key.to_string(),
            evicted_messages: processed.evicted,
            system_prompts: Vec::new(),
            summary,
            memory: None,
            metadata: std::mem::take(&mut metadata),
        };

        // Inject pre-loaded system prompt directly
        if !self.system_prompt.is_empty() {
            ctx_prepare.system_prompts.push(self.system_prompt.clone());
        }
        // Inject pre-loaded skills context directly
        if let Some(ref skills) = self.skills_context {
            ctx_prepare.system_prompts.push(skills.clone());
        }
        // Run remaining hooks (logging, custom extensions)
        self.hooks.run_on_context_prepare(&mut ctx_prepare).await;

        // ── 7. Read long-term memory (direct via trait) ────────────
        if ctx_prepare.memory.is_none() {
            match self.memory.read_long_term().await {
                Ok(mem) if !mem.is_empty() => {
                    ctx_prepare.memory = Some(mem);
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("Failed to read long-term memory: {}", e);
                }
            }
        }

        // ── 8. Assemble prompt (pure, synchronous) ─────────────────
        let messages = Self::assemble_prompt(
            processed.messages,
            content,
            &ctx_prepare.system_prompts,
            ctx_prepare.memory.as_deref(),
            ctx_prepare.summary.as_deref(),
        );

        // ── 9. Run agent loop ─────────────────────────────────────
        let effective_cb = if self.config.streaming {
            callback
        } else {
            None
        };
        let result = self
            .run_agent_loop(messages, effective_cb, ctx_prepare.metadata, &request_id)
            .await?;

        // ── 10. Hook: on_response ─────────────────────────────────
        let AgentLoopResult {
            content: loop_content,
            reasoning_content: loop_reasoning,
            tools_used: loop_tools,
            metadata: loop_metadata,
        } = result;

        let mut resp_ctx = ResponseContext {
            request_id: request_id.clone(),
            content: loop_content,
            reasoning_content: loop_reasoning,
            tools_used: loop_tools,
            session_key: session_key.to_string(),
            metadata: loop_metadata,
        };
        self.hooks.run_on_response(&mut resp_ctx).await;

        // ── 11. Save assistant message (direct via trait) ──────────
        if let Err(e) = self
            .sessions
            .append_by_key(
                session_key,
                "assistant",
                &resp_ctx.content,
                Some(resp_ctx.tools_used.clone()),
            )
            .await
        {
            warn!("Failed to persist assistant message: {}", e);
        }

        Ok(AgentResponse {
            content: resp_ctx.content,
            reasoning_content: resp_ctx.reasoning_content,
            tools_used: resp_ctx.tools_used,
        })
    }

    // ── Agent Loop Internals ────────────────────────────────

    /// Unified agent iteration loop.
    ///
    /// Always uses `chat_stream` — for non-streaming providers the default
    /// trait impl wraps the response in a single-chunk stream, so both paths
    /// converge here.  When `callback` is `None`, stream events are silently
    /// discarded.
    #[instrument(name = "agent.run_loop", skip_all, fields(model = %self.config.model))]
    async fn run_agent_loop(
        &self,
        initial_messages: Vec<ChatMessage>,
        callback: Option<&StreamCallback>,
        mut hook_metadata: HashMap<String, serde_json::Value>,
        request_id: &str,
    ) -> Result<AgentLoopResult, AgentError> {
        let noop: StreamCallback = Box::new(|_| {});
        let cb: &StreamCallback = callback.unwrap_or(&noop);

        let mut messages = initial_messages;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, &self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("Agent loop iteration {}", iteration);

            // ── Hook: on_llm_request ──────────────────────────
            let mut llm_req_ctx = LlmRequestContext {
                request_id: request_id.to_string(),
                messages: messages.clone(),
                iteration,
                metadata: hook_metadata.clone(),
            };
            self.hooks.run_on_llm_request(&mut llm_req_ctx).await;
            messages = llm_req_ctx.messages;
            hook_metadata = llm_req_ctx.metadata;

            let request = request_handler.build_chat_request(&messages);
            let mut stream = request_handler.send_with_retry(request).await?;
            let response = stream::accumulate_stream(&mut stream, cb).await?;

            // ── Hook: on_llm_response ─────────────────────────
            let mut llm_resp_ctx = LlmResponseContext {
                request_id: request_id.to_string(),
                response: response.clone(),
                iteration,
                metadata: hook_metadata.clone(),
            };
            self.hooks.run_on_llm_response(&mut llm_resp_ctx).await;
            let response = llm_resp_ctx.response;
            hook_metadata = llm_resp_ctx.metadata;

            if !response.has_tool_calls() {
                let content = response.content.unwrap_or_else(|| {
                    "I've completed processing but have no response to give.".to_string()
                });
                cb(&StreamEvent::Done);
                return Ok(AgentLoopResult {
                    content,
                    reasoning_content: response.reasoning_content,
                    tools_used,
                    metadata: hook_metadata,
                });
            }

            // Has tool calls — execute them and continue the loop
            let mut state = ToolCallState {
                messages: &mut messages,
                tools_used: &mut tools_used,
                hook_metadata: &mut hook_metadata,
            };
            self.handle_tool_calls(&response, &executor, &mut state, cb, request_id)
                .await;
        }

        // Exhausted max iterations without a final response
        Ok(AgentLoopResult {
            content: "I've completed processing but have no response to give.".to_string(),
            reasoning_content: None,
            tools_used,
            metadata: hook_metadata,
        })
    }

    /// Execute tool calls, append results to messages, and update tracking.
    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        state: &mut ToolCallState<'_>,
        cb: &StreamCallback,
        request_id: &str,
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

        // Execute tool calls one by one, honoring hooks
        for tool_call in &response.tool_calls {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // ── Hook: on_tool_execute ──────────────────────────
            let mut tool_ctx = ToolExecuteContext {
                request_id: request_id.to_string(),
                tool_name: tool_name.clone(),
                tool_args: tool_args.clone(),
                skip: false,
                skip_result: None,
                metadata: state.hook_metadata.clone(),
            };
            self.hooks.run_on_tool_execute(&mut tool_ctx).await;
            *state.hook_metadata = tool_ctx.metadata;

            let start = std::time::Instant::now();

            let output = if tool_ctx.skip {
                tool_ctx
                    .skip_result
                    .unwrap_or_else(|| "[skipped by hook]".to_string())
            } else {
                let result = executor.execute_one(tool_call).await;
                result.output
            };

            let duration_ms = start.elapsed().as_millis() as u64;

            // ── Hook: on_tool_result ───────────────────────────
            let mut result_ctx = ToolResultContext {
                request_id: request_id.to_string(),
                tool_name: tool_name.clone(),
                tool_result: output.clone(),
                duration_ms,
                metadata: state.hook_metadata.clone(),
            };
            self.hooks.run_on_tool_result(&mut result_ctx).await;
            *state.hook_metadata = result_ctx.metadata;

            state.tools_used.push(tool_name.clone());

            cb(&StreamEvent::ToolStart {
                name: tool_name.clone(),
            });

            let output_preview = truncate_preview(&result_ctx.tool_result, 500);

            cb(&StreamEvent::ToolEnd {
                name: tool_name.clone(),
                output: output_preview,
            });
            // Add the tool result to the conversation
            state.messages.push(ChatMessage::tool_result(
                tool_call.id.clone(),
                tool_name.clone(),
                result_ctx.tool_result.clone(),
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
            match msg.role.as_str() {
                "user" => messages.push(ChatMessage::user(&msg.content)),
                "assistant" => messages.push(ChatMessage::assistant(&msg.content)),
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
