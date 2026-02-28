//! Agent loop: the core processing engine

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

use crate::agent::context::ContextBuilder;
use crate::agent::executor::ToolExecutor;
use crate::agent::memory::MemoryStore;
use crate::agent::request::RequestHandler;
use crate::agent::skill_loader;
use crate::agent::stream::{self, StreamCallback, StreamEvent};
use crate::providers::{ChatMessage, ChatResponse, LlmProvider};
use crate::session::SessionManager;
use crate::tools::ToolRegistry;

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

/// The agent loop - core processing engine
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    context: ContextBuilder,
    memory: MemoryStore,
    sessions: SessionManager,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// The caller is responsible for constructing and populating the
    /// `ToolRegistry` before passing it in — this keeps the agent loop
    /// decoupled from specific tool implementations.
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

        // Load skills
        let skills_context = skill_loader::load_skills(&workspace).await;

        // Build context with skills and summarization support
        let store_arc = Arc::new(memory.sqlite_store().clone());
        let context = ContextBuilder::new(workspace.clone())?
            .with_skills_context(skills_context)
            .with_summarization(provider.clone(), store_arc, config.model.clone());

        Ok(Self {
            provider,
            context,
            memory,
            sessions,
            tools,
            config,
            workspace,
        })
    }

    /// Create a new agent loop with a pre-built, cached context builder.
    ///
    /// This is the preferred constructor for subagents to avoid repeated
    /// synchronous file I/O. The context builder should be created once
    /// at startup and shared via `Arc`.
    ///
    /// # Note
    ///
    /// This constructor performs **no synchronous file I/O** and is safe
    /// to call in async contexts.
    pub async fn with_cached_context(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        context: ContextBuilder,
    ) -> Result<Self> {
        let memory = MemoryStore::new().await;
        let sessions = SessionManager::new(memory.sqlite_store().clone());

        Ok(Self {
            provider,
            context,
            memory,
            sessions,
            tools,
            config,
            workspace,
        })
    }

    /// Get the cached context builder for sharing with subagents.
    pub fn context_builder(&self) -> &ContextBuilder {
        &self.context
    }

    /// Get the model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
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
        let mut session = self.sessions.get_or_create(session_key).await;

        // Handle slash commands
        let cmd = content.trim().to_lowercase();
        if cmd == "/new" {
            session.clear();
            self.sessions.save(&session).await;
            return Ok(AgentResponse {
                content: "New session started.".to_string(),
                reasoning_content: None,
                tools_used: Vec::new(),
            });
        }
        if cmd == "/help" {
            return Ok(AgentResponse {
                content: "🐈 nanobot commands:\n/new — Start a new conversation\n/help — Show available commands".to_string(),
                reasoning_content: None,
                tools_used: Vec::new(),
            });
        }

        // Save user message BEFORE calling LLM so it persists even if
        // the LLM call fails or the process is interrupted.
        // Capture history BEFORE appending so build_messages won't duplicate.
        let history_snapshot = session.get_history(self.config.memory_window);

        if let Err(e) = self
            .sessions
            .append_message(&mut session, "user", content, None)
            .await
        {
            warn!("Failed to persist user message to SQLite: {}", e);
        }
        if let Err(e) = self
            .memory
            .append_history(&format!("User: {}", content))
            .await
        {
            warn!("Failed to persist user history to SQLite: {}", e);
        }

        // Build messages using the history snapshot (without the just-appended user message)
        let memory_content = self.memory.read_long_term().await.ok();
        let messages = self
            .context
            .build_messages(
                history_snapshot,
                content,
                memory_content.as_deref(),
                "cli",
                "direct",
                session_key,
            )
            .await;

        // Run the agent loop — always uses streaming internally; when callback
        // is absent, stream events are silently discarded.
        let effective_cb = if self.config.streaming {
            callback
        } else {
            None
        };
        let (response, reasoning, tools_used) = self.run_agent_loop(messages, effective_cb).await?;

        // Save assistant response AFTER LLM call completes
        if let Err(e) = self
            .sessions
            .append_message(
                &mut session,
                "assistant",
                &response,
                Some(tools_used.clone()),
            )
            .await
        {
            warn!("Failed to persist assistant message to SQLite: {}", e);
        }
        if let Err(e) = self
            .memory
            .append_history(&format!("Assistant: {}", response))
            .await
        {
            warn!("Failed to persist assistant history to SQLite: {}", e);
        }

        Ok(AgentResponse {
            content: response,
            reasoning_content: reasoning,
            tools_used,
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
    ) -> Result<(String, Option<String>, Vec<String>)> {
        let noop: StreamCallback = Box::new(|_| {});
        let cb: &StreamCallback = callback.unwrap_or(&noop);

        let mut messages = initial_messages;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);
        let request_handler = RequestHandler::new(&self.provider, &self.tools, &self.config);

        for iteration in 1..=self.config.max_iterations {
            debug!("Agent loop iteration {}", iteration);

            let request = request_handler.build_chat_request(&messages);
            let mut stream = request_handler.send_with_retry(request).await?;
            let response = stream::accumulate_stream(&mut stream, cb).await?;

            if !response.has_tool_calls() {
                Self::log_response(&response);
                let content = response.content.unwrap_or_else(|| {
                    "I've completed processing but have no response to give.".to_string()
                });
                cb(&StreamEvent::Done);
                return Ok((content, response.reasoning_content, tools_used));
            }

            // Has tool calls — execute them and continue the loop
            Self::log_response(&response);
            self.handle_tool_calls(&response, &executor, &mut messages, &mut tools_used, cb)
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
        self.context.add_assistant_message(
            messages,
            response.content.clone(),
            response
                .tool_calls
                .iter()
                .map(|tc| serde_json::to_value(tc).unwrap_or_default())
                .collect(),
            response.reasoning_content.clone(),
        );
        if let Some(last) = messages.last_mut() {
            last.tool_calls = Some(response.tool_calls.clone());
        }

        // Execute and collect results
        let results = executor.execute_batch(&response.tool_calls).await;
        for result in &results {
            tools_used.push(result.tool_name.clone());

            cb(&StreamEvent::ToolStart {
                name: result.tool_name.clone(),
            });

            let output_preview = truncate_preview(&result.output, 500);
            debug!("[Tool] {} -> {}", result.tool_name, output_preview);

            cb(&StreamEvent::ToolEnd {
                name: result.tool_name.clone(),
                output: output_preview,
            });

            self.context.add_tool_result(
                messages,
                result.tool_call_id.clone(),
                result.tool_name.clone(),
                result.output.clone(),
            );
        }
    }

    /// Log reasoning and content from a response (shared by tool-call and final branches).
    fn log_response(response: &ChatResponse) {
        if let Some(ref reasoning) = response.reasoning_content {
            if !reasoning.is_empty() {
                debug!("[Agent] Reasoning: {}", reasoning);
            }
        }
        if let Some(ref content) = response.content {
            if !content.is_empty() {
                info!("[Agent] Response: {}", content);
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────

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
