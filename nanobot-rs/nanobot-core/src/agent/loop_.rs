//! Agent loop: the core processing engine

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tracing::{debug, info, instrument, warn};

use crate::agent::context::ContextBuilder;
use crate::agent::executor::ToolExecutor;
use crate::agent::memory::MemoryStore;
use crate::providers::{
    parse_json_args, ChatMessage, ChatRequest, ChatResponse, LlmProvider, ThinkingConfig, ToolCall,
};
use crate::session::SessionManager;
use crate::skills::{SkillsLoader, SkillsRegistry};
use crate::tools::ToolRegistry;

/// Callback type for streaming output.
///
/// Called for each chunk of text or reasoning content as it arrives.
pub type StreamCallback = Box<dyn Fn(&StreamEvent) + Send + Sync>;

/// Events emitted during streaming.
#[derive(Debug)]
pub enum StreamEvent {
    /// Incremental text content
    Content(String),
    /// Incremental reasoning/thinking content
    Reasoning(String),
    /// A tool is being called
    ToolStart { name: String },
    /// Tool execution finished
    ToolEnd { name: String, output: String },
    /// Stream completed
    Done,
}

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
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
    ) -> Result<Self> {
        let memory = MemoryStore::new();
        let sessions = SessionManager::new(memory.sqlite_store().clone());

        // Load skills
        let skills_context = Self::load_skills(&workspace);

        // Build context with skills
        let context = ContextBuilder::new(workspace.clone())?.with_skills_context(skills_context);

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
    pub fn with_cached_context(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        context: ContextBuilder,
    ) -> Result<Self> {
        let memory = MemoryStore::new();
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

    /// Load skills from builtin and user directories
    fn load_skills(workspace: &Path) -> Option<String> {
        let user_skills_dir = workspace.join("skills");

        // Locate builtin skills: try relative to the executable, then a few common fallbacks
        let builtin_skills_dir = Self::find_builtin_skills_dir();

        let builtin_dir = match builtin_skills_dir {
            Some(dir) => dir,
            None => {
                debug!("Built-in skills directory not found, loading user skills only");
                // Still try loading user skills
                if !user_skills_dir.exists() {
                    debug!("No skills directories found");
                    return None;
                }
                PathBuf::from("/nonexistent")
            }
        };

        let loader = SkillsLoader::new(user_skills_dir, builtin_dir);
        match SkillsRegistry::from_loader(loader) {
            Ok(registry) => {
                let summary = registry.generate_context_summary();
                if summary.is_empty() {
                    info!("No skills loaded");
                    None
                } else {
                    info!(
                        "Loaded {} skills ({} available)",
                        registry.len(),
                        registry.list_available().len()
                    );
                    Some(summary)
                }
            }
            Err(e) => {
                warn!("Failed to load skills: {}", e);
                None
            }
        }
    }

    /// Find the builtin skills directory
    fn find_builtin_skills_dir() -> Option<PathBuf> {
        // Try relative to the executable
        if let Ok(exe) = std::env::current_exe() {
            // dev build: target/debug/nanobot → nanobot-core/skills/
            if let Some(project_root) = exe
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
            {
                let candidate = project_root.join("nanobot-core").join("skills");
                if candidate.exists() {
                    debug!("Found builtin skills at {:?}", candidate);
                    return Some(candidate);
                }
            }
        }

        // Try current working directory
        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join("nanobot-core").join("skills");
            if candidate.exists() {
                debug!("Found builtin skills at {:?}", candidate);
                return Some(candidate);
            }
            // Also try if we're inside nanobot-core
            let candidate = cwd.join("skills");
            if candidate.exists() {
                debug!("Found builtin skills at {:?}", candidate);
                return Some(candidate);
            }
        }

        None
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

        // Build messages
        let memory_content = self.memory.read_long_term().await.ok();
        let messages = self.context.build_messages(
            session.get_history(self.config.memory_window),
            content,
            memory_content.as_deref(),
            "cli",
            "direct",
        );

        // Run the agent loop (streaming or non-streaming)
        let (response, reasoning, tools_used) = if self.config.streaming && callback.is_some() {
            self.run_agent_loop_streaming(messages, callback.unwrap())
                .await?
        } else {
            self.run_agent_loop(messages).await?
        };

        // Save to session using O(1) append operations
        self.sessions
            .append_message(&mut session, "user", content, None)
            .await;
        self.sessions
            .append_message(
                &mut session,
                "assistant",
                &response,
                Some(tools_used.clone()),
            )
            .await;

        Ok(AgentResponse {
            content: response,
            reasoning_content: reasoning,
            tools_used,
        })
    }

    /// Run the agent iteration loop
    #[instrument(name = "agent.run_loop", skip_all, fields(model = %self.config.model))]
    async fn run_agent_loop(
        &self,
        initial_messages: Vec<ChatMessage>,
    ) -> Result<(String, Option<String>, Vec<String>)> {
        let mut messages = initial_messages;
        let mut iteration = 0;
        let mut final_content = None;
        let mut final_reasoning = None;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);

        let model_name = Arc::new(self.config.model.clone());

        while iteration < self.config.max_iterations {
            iteration += 1;
            debug!("Agent loop iteration {}", iteration);

            let request = ChatRequest {
                model: model_name.to_string(),
                messages: messages.clone(),
                tools: Some(self.tools.get_definitions()),
                temperature: Some(self.config.temperature),
                max_tokens: Some(self.config.max_tokens),
                thinking: if self.config.thinking_enabled {
                    Some(ThinkingConfig::enabled())
                } else {
                    None
                },
            };

            let mut retries = 0;
            let max_retries = 3;
            let response = loop {
                match self.provider.chat(request.clone()).await {
                    Ok(resp) => break resp,
                    Err(e) => {
                        if retries >= max_retries {
                            return Err(e.context("Provider API request failed after retries"));
                        }
                        warn!(
                            "Provider error: {}. Retrying {}/{}",
                            e,
                            retries + 1,
                            max_retries
                        );
                        retries += 1;
                        tokio::time::sleep(std::time::Duration::from_secs(2_u64.pow(retries)))
                            .await;
                    }
                }
            };

            if response.has_tool_calls() {
                // Log reasoning content if present
                if let Some(ref reasoning) = response.reasoning_content {
                    if !reasoning.is_empty() {
                        info!("[Agent] Reasoning: {}", reasoning);
                    }
                }

                // Log LLM response content if present
                if let Some(ref content) = response.content {
                    if !content.is_empty() {
                        info!("[Agent] Response: {}", content);
                    }
                }

                // Log tool calls being executed
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

                // Add assistant message with tool calls (via ContextBuilder)
                self.context.add_assistant_message(
                    &mut messages,
                    response.content.clone(),
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| serde_json::to_value(tc).unwrap_or_default())
                        .collect(),
                    response.reasoning_content.clone(),
                );
                // Re-add the tool_calls on the last assistant message so the
                // provider can see them in the next request
                if let Some(last) = messages.last_mut() {
                    last.tool_calls = Some(response.tool_calls.clone());
                }

                // Execute each tool call via ToolExecutor
                let results = executor.execute_batch(&response.tool_calls).await;
                for result in &results {
                    tools_used.push(result.tool_name.clone());

                    // Log tool execution result
                    let output_preview = if result.output.len() > 500 {
                        // Find a valid UTF-8 char boundary near byte 500
                        let end = result
                            .output
                            .char_indices()
                            .take_while(|(i, _)| *i < 500)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(0);
                        format!(
                            "{}... (truncated, {} chars total)",
                            &result.output[..end],
                            result.output.len()
                        )
                    } else {
                        result.output.clone()
                    };
                    info!("[Tool] {} -> {}", result.tool_name, output_preview);

                    self.context.add_tool_result(
                        &mut messages,
                        result.tool_call_id.clone(),
                        result.tool_name.clone(),
                        result.output.clone(),
                    );
                }

                // No reflection message — the LLM already has the tool results
                // and will decide next steps on its own.
            } else {
                // Log final response
                if let Some(ref reasoning) = response.reasoning_content {
                    if !reasoning.is_empty() {
                        info!("[Agent] Reasoning: {}", reasoning);
                    }
                }
                if let Some(ref content) = response.content {
                    if !content.is_empty() {
                        info!("[Agent] Final response: {}", content);
                    }
                }
                final_content = response.content;
                final_reasoning = response.reasoning_content;
                break;
            }
        }

        let content = final_content.unwrap_or_else(|| {
            "I've completed processing but have no response to give.".to_string()
        });

        Ok((content, final_reasoning, tools_used))
    }

    /// Streaming variant of `run_agent_loop`.
    ///
    /// Uses `chat_stream` and emits `StreamEvent`s via the callback. Accumulates
    /// chunks into a complete `ChatResponse` for tool execution and history.
    #[instrument(name = "agent.run_loop_streaming", skip_all, fields(model = %self.config.model))]
    async fn run_agent_loop_streaming(
        &self,
        initial_messages: Vec<ChatMessage>,
        callback: &StreamCallback,
    ) -> Result<(String, Option<String>, Vec<String>)> {
        let mut messages = initial_messages;
        let mut iteration = 0;
        let mut final_content = None;
        let mut final_reasoning = None;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);

        let model_name = Arc::new(self.config.model.clone());

        while iteration < self.config.max_iterations {
            iteration += 1;
            debug!("Agent loop streaming iteration {}", iteration);

            let request = ChatRequest {
                model: model_name.to_string(),
                messages: messages.clone(),
                tools: Some(self.tools.get_definitions()),
                temperature: Some(self.config.temperature),
                max_tokens: Some(self.config.max_tokens),
                thinking: if self.config.thinking_enabled {
                    Some(ThinkingConfig::enabled())
                } else {
                    None
                },
            };

            // Get the stream
            let mut retries = 0;
            let max_retries = 3;
            let mut stream = loop {
                match self.provider.chat_stream(request.clone()).await {
                    Ok(s) => break s,
                    Err(e) => {
                        if retries >= max_retries {
                            return Err(
                                e.context("Provider streaming request failed after retries")
                            );
                        }
                        warn!(
                            "Provider stream error: {}. Retrying {}/{}",
                            e,
                            retries + 1,
                            max_retries
                        );
                        retries += 1;
                        tokio::time::sleep(std::time::Duration::from_secs(2_u64.pow(retries)))
                            .await;
                    }
                }
            };

            // Accumulate chunks into a full response
            let response = Self::accumulate_stream(&mut stream, callback).await?;

            if response.has_tool_calls() {
                // Log reasoning content if present
                if let Some(ref reasoning) = response.reasoning_content {
                    if !reasoning.is_empty() {
                        info!("[Agent] Reasoning: {}", reasoning);
                    }
                }

                // Log LLM response content if present
                if let Some(ref content) = response.content {
                    if !content.is_empty() {
                        info!("[Agent] Response: {}", content);
                    }
                }

                // Log tool calls being executed
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

                // Add assistant message with tool calls
                self.context.add_assistant_message(
                    &mut messages,
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

                // Execute each tool call
                let results = executor.execute_batch(&response.tool_calls).await;
                for result in &results {
                    tools_used.push(result.tool_name.clone());

                    callback(&StreamEvent::ToolStart {
                        name: result.tool_name.clone(),
                    });

                    let output_preview = if result.output.len() > 500 {
                        // Find a valid UTF-8 char boundary near byte 500
                        let end = result
                            .output
                            .char_indices()
                            .take_while(|(i, _)| *i < 500)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(0);
                        format!(
                            "{}... (truncated, {} chars total)",
                            &result.output[..end],
                            result.output.len()
                        )
                    } else {
                        result.output.clone()
                    };
                    info!("[Tool] {} -> {}", result.tool_name, output_preview);

                    callback(&StreamEvent::ToolEnd {
                        name: result.tool_name.clone(),
                        output: output_preview,
                    });

                    self.context.add_tool_result(
                        &mut messages,
                        result.tool_call_id.clone(),
                        result.tool_name.clone(),
                        result.output.clone(),
                    );
                }
            } else {
                // Final response
                if let Some(ref reasoning) = response.reasoning_content {
                    if !reasoning.is_empty() {
                        info!("[Agent] Reasoning: {}", reasoning);
                    }
                }
                if let Some(ref content) = response.content {
                    if !content.is_empty() {
                        info!("[Agent] Final response: {}", content);
                    }
                }
                final_content = response.content;
                final_reasoning = response.reasoning_content;
                callback(&StreamEvent::Done);
                break;
            }
        }

        let content = final_content.unwrap_or_else(|| {
            "I've completed processing but have no response to give.".to_string()
        });

        Ok((content, final_reasoning, tools_used))
    }

    /// Consume a stream, emitting events via callback, and return the
    /// accumulated complete `ChatResponse`.
    async fn accumulate_stream(
        stream: &mut crate::providers::ChatStream,
        callback: &StreamCallback,
    ) -> Result<ChatResponse> {
        use std::collections::HashMap;

        let mut content = String::new();
        let mut reasoning_content = String::new();

        // Tool call accumulation: index -> (id, name, arguments)
        let mut tool_calls_map: HashMap<usize, (String, String, String)> = HashMap::new();

        // Log streaming start
        info!("[LLM Streaming] <<<START>>>");

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;

            // Accumulate text content
            if let Some(ref text) = chunk.delta.content {
                if !text.is_empty() {
                    content.push_str(text);
                    callback(&StreamEvent::Content(text.clone()));
                }
            }

            // Accumulate reasoning content
            if let Some(ref reasoning) = chunk.delta.reasoning_content {
                if !reasoning.is_empty() {
                    reasoning_content.push_str(reasoning);
                    callback(&StreamEvent::Reasoning(reasoning.clone()));
                }
            }

            // Accumulate tool calls
            for tc_delta in &chunk.delta.tool_calls {
                let entry = tool_calls_map
                    .entry(tc_delta.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));

                if let Some(ref id) = tc_delta.id {
                    entry.0 = id.clone();
                }
                if let Some(ref name) = tc_delta.function_name {
                    entry.1 = name.clone();
                }
                if let Some(ref args) = tc_delta.function_arguments {
                    entry.2.push_str(args);
                }
            }
        }

        // Log streaming end with summary
        info!("[LLM Streaming] <<<END>>>");

        // Convert accumulated tool calls into ToolCall objects
        let mut tool_calls: Vec<ToolCall> = tool_calls_map
            .into_iter()
            .map(|(_, (id, name, args))| {
                let arguments = parse_json_args(&args);
                ToolCall::new(id, name, arguments)
            })
            .collect();
        tool_calls.sort_by_key(|tc| tc.id.clone());

        Ok(ChatResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
            reasoning_content: if reasoning_content.is_empty() {
                None
            } else {
                Some(reasoning_content)
            },
        })
    }
}
