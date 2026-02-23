//! Agent loop: the core processing engine

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

use crate::agent::context::ContextBuilder;
use crate::agent::executor::ToolExecutor;
use crate::agent::memory::MemoryStore;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider, ThinkingConfig};
use crate::session::SessionManager;
use crate::skills::{SkillsLoader, SkillsRegistry};
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

        // Run the agent loop
        let (response, reasoning, tools_used) = self.run_agent_loop(messages).await?;

        // Save to session
        session.add_message("user", content, None);
        session.add_message("assistant", &response, None);
        self.sessions.save(&session).await;

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
                        format!(
                            "{}... (truncated, {} chars total)",
                            &result.output[..500],
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
}
