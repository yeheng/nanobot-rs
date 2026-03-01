//! Agent loop: the core processing engine

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

use crate::agent::executor::ToolExecutor;
use crate::agent::history_processor::{process_history, HistoryConfig};
use crate::agent::request::RequestHandler;
use crate::agent::stream::{self, StreamCallback, StreamEvent};
use crate::agent::summarization::SummarizationService;
use crate::hooks::logging::LoggingHook;
use crate::hooks::persistence::PersistenceHook;
use crate::hooks::prompt::{BootstrapHook, SkillsHook};
use crate::hooks::summarization::SummarizationHook;
use crate::hooks::{
    AgentHook, ContextPrepareContext, HookRegistry, LlmRequestContext, LlmResponseContext,
    RequestContext, ResponseContext, SessionLoadContext, SessionSaveContext, ToolExecuteContext,
    ToolResultContext,
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

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// After decoupling, the agent loop only holds:
/// - **LLM provider** — for making chat calls
/// - **ContextBuilder** — pure data assembler for prompts
/// - **ToolRegistry** — registered tool definitions
/// - **HookRegistry** — all extensions (persistence, summarization, etc.)
/// - **Config** — model, iteration limits, etc.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
    /// Lifecycle hooks for extending agent behavior.
    hooks: HookRegistry,
    /// History truncator configuration.
    history_config: HistoryConfig,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Automatically registers default hooks:
    /// - **PersistenceHook** — session + memory I/O
    /// - **SummarizationHook** — LLM-based context compression
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
    ) -> Result<Self> {
        let memory = MemoryStore::new().await;
        let sessions = SessionManager::new(memory.sqlite_store().clone());

        // Create default hooks
        let store_arc = Arc::new(memory.sqlite_store().clone());
        let summarization_service =
            SummarizationService::new(provider.clone(), store_arc, config.model.clone());

        let mut hooks = HookRegistry::new();
        hooks.register(Arc::new(PersistenceHook::new(sessions, memory)));
        hooks.register(Arc::new(SummarizationHook::new(summarization_service)));
        hooks.register(Arc::new(LoggingHook::new()));

        // Add context building hooks
        hooks.register(Arc::new(BootstrapHook::new_full(&workspace).await?));
        hooks.register(Arc::new(SkillsHook::new(&workspace).await));

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            hooks,
            history_config: HistoryConfig::default(),
        })
    }

    /// Create a new agent loop for subagents without default hooks.
    ///
    /// Callers are responsible for registering hooks as needed (e.g. `BootstrapHook::new_minimal`).
    pub async fn builder(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
    ) -> Result<Self> {
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
        })
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
    ///
    /// Requires that a `PersistenceHook` is registered (which is the case
    /// when using `AgentLoop::new()`).
    pub async fn clear_session(&self, session_key: &str) {
        // Access the PersistenceHook's SessionManager
        if let Some(persistence) = self.hooks.get_hook::<PersistenceHook>() {
            let mut session = persistence.sessions().get_or_create(session_key).await;
            session.clear();
            persistence.sessions().save(&session).await;
        } else {
            warn!("clear_session called but no PersistenceHook is registered");
        }
    }

    /// Process a message directly (for CLI or testing)
    #[instrument(skip(self, content))]
    pub async fn process_direct(&self, content: &str, session_key: &str) -> Result<AgentResponse> {
        self.process_direct_with_callback(content, session_key, None)
            .await
    }

    /// Process a message with optional streaming callback.
    ///
    /// When `callback` is `Some` and `config.streaming` is enabled, the agent
    /// loop will use streaming LLM calls and emit `StreamEvent`s via the
    /// callback as content arrives.
    #[instrument(skip(self, content, callback))]
    pub async fn process_direct_with_callback(
        &self,
        content: &str,
        session_key: &str,
        callback: Option<&StreamCallback>,
    ) -> Result<AgentResponse> {
        // ── Hook: on_request ───────────────────────────────────
        let mut req_ctx = RequestContext {
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

        // ── Hook: on_session_load ──────────────────────────────
        let mut load_ctx = SessionLoadContext {
            session_key: session_key.to_string(),
            memory_window: self.config.memory_window,
            history: Vec::new(),
            metadata: req_ctx.metadata.clone(),
        };
        self.hooks.run_on_session_load(&mut load_ctx).await;
        let history_snapshot = load_ctx.history;

        // ── Hook: on_session_save (user message) ───────────────
        let mut save_ctx = SessionSaveContext {
            session_key: session_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            tools_used: None,
            metadata: load_ctx.metadata.clone(),
        };
        self.hooks.run_on_session_save(&mut save_ctx).await;

        // ── Three-step pipeline ────────────────────────────────
        // 1. Truncate history (pure computation)
        let processed = process_history(history_snapshot, &self.history_config);

        let mut ctx_prepare = ContextPrepareContext {
            session_key: session_key.to_string(),
            evicted_messages: processed.evicted,
            system_prompts: Vec::new(),
            summary: None,
            memory: None,
            metadata: save_ctx.metadata.clone(),
        };
        self.hooks.run_on_context_prepare(&mut ctx_prepare).await;

        // 3. Assemble prompt (pure, synchronous)
        let messages = Self::assemble_prompt(
            processed.messages,
            content,
            &ctx_prepare.system_prompts,
            ctx_prepare.memory.as_deref(),
            ctx_prepare.summary.as_deref(),
        );

        // Run the agent loop
        let effective_cb = if self.config.streaming {
            callback
        } else {
            None
        };
        let (response, reasoning, tools_used) = self
            .run_agent_loop(messages, effective_cb, ctx_prepare.metadata)
            .await?;

        // ── Hook: on_response ─────────────────────────────────
        let mut resp_ctx = ResponseContext {
            content: response.clone(),
            reasoning_content: reasoning.clone(),
            tools_used: tools_used.clone(),
            session_key: session_key.to_string(),
            metadata: HashMap::new(),
        };
        self.hooks.run_on_response(&mut resp_ctx).await;

        // ── Hook: on_session_save (assistant response) ─────────
        let mut save_ctx = SessionSaveContext {
            session_key: session_key.to_string(),
            role: "assistant".to_string(),
            content: resp_ctx.content.clone(),
            tools_used: Some(resp_ctx.tools_used.clone()),
            metadata: resp_ctx.metadata.clone(),
        };
        self.hooks.run_on_session_save(&mut save_ctx).await;

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
    ) -> Result<(String, Option<String>, Vec<String>)> {
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
                return Ok((content, response.reasoning_content, tools_used));
            }

            // Has tool calls — execute them and continue the loop
            self.handle_tool_calls(
                &response,
                &executor,
                &mut messages,
                &mut tools_used,
                cb,
                &mut hook_metadata,
            )
            .await;
        }

        // Exhausted max iterations without a final response
        Ok((
            "I've completed processing but have no response to give.".to_string(),
            None,
            tools_used,
        ))
    }

    /// Execute tool calls, append results to messages, and update tracking.
    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        messages: &mut Vec<ChatMessage>,
        tools_used: &mut Vec<String>,
        cb: &StreamCallback,
        hook_metadata: &mut HashMap<String, serde_json::Value>,
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
                messages.push(ChatMessage::assistant(c));
            }
        } else {
            messages.push(ChatMessage::assistant_with_tools(
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
                tool_name: tool_name.clone(),
                tool_args: tool_args.clone(),
                skip: false,
                skip_result: None,
                metadata: hook_metadata.clone(),
            };
            self.hooks.run_on_tool_execute(&mut tool_ctx).await;
            *hook_metadata = tool_ctx.metadata;

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
                tool_name: tool_name.clone(),
                tool_result: output.clone(),
                duration_ms,
                metadata: hook_metadata.clone(),
            };
            self.hooks.run_on_tool_result(&mut result_ctx).await;
            *hook_metadata = result_ctx.metadata;

            tools_used.push(tool_name.clone());

            cb(&StreamEvent::ToolStart {
                name: tool_name.clone(),
            });

            let output_preview = truncate_preview(&result_ctx.tool_result, 500);

            cb(&StreamEvent::ToolEnd {
                name: tool_name.clone(),
                output: output_preview,
            });
            // Add the tool result to the conversation
            messages.push(ChatMessage::tool_result(
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
