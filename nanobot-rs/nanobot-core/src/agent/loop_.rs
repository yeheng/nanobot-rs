//! Agent loop: the core processing engine

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument, warn};

use crate::agent::context::ContextBuilder;
use crate::agent::executor::ToolExecutor;
use crate::agent::memory::MemoryStore;
use crate::bus::MessageBus;
use crate::cron::CronService;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use crate::session::SessionManager;
use crate::skills::{SkillsLoader, SkillsRegistry};
use crate::tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MessageTool, ReadFileTool, SpawnTool,
    ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};
use crate::tools::middleware::{ToolInvocation, ToolLoggingMiddleware};
use crate::trail::{Middleware, MiddlewareStack, TrailContext};

/// Agent loop configuration
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub memory_window: usize,
    pub restrict_to_workspace: bool,
    /// Maximum characters for tool result output (0 = unlimited)
    pub max_tool_result_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            max_iterations: 20,
            temperature: 0.7,
            max_tokens: 4096,
            memory_window: 50,
            restrict_to_workspace: false,
            max_tool_result_chars: 8000,
        }
    }
}

/// Optional dependencies for AgentLoop that enable additional tools.
///
/// When provided, the corresponding tools are registered automatically.
pub struct AgentDependencies {
    /// Message bus for the `send_message` tool
    pub bus: Option<Arc<MessageBus>>,
    /// Cron service for the `cron` tool
    pub cron_service: Option<Arc<CronService>>,
    /// Web tools configuration
    pub web_tools: Option<crate::config::WebToolsConfig>,
    /// Pre-started MCP tool bridges (created via `mcp::start_mcp_servers`)
    pub mcp_tools: Vec<Box<dyn crate::tools::Tool>>,
    /// Tool execution middlewares (logging, permission, timeout, etc.)
    pub tool_middleware: Vec<Arc<dyn Middleware<ToolInvocation, String> + Send + Sync>>,
}

impl Default for AgentDependencies {
    fn default() -> Self {
        Self {
            bus: None,
            cron_service: None,
            web_tools: None,
            mcp_tools: Vec::new(),
            tool_middleware: Vec::new(),
        }
    }
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
    tool_middleware: MiddlewareStack<ToolInvocation, String>,
}

impl AgentLoop {
    /// Create a new agent loop
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
    ) -> Result<Self> {
        Self::with_dependencies(provider, workspace, config, AgentDependencies::default())
    }

    /// Create a new agent loop with optional dependencies for extra tools
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub fn with_dependencies(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        deps: AgentDependencies,
    ) -> Result<Self> {
        let memory = MemoryStore::new(workspace.clone());
        let sessions = SessionManager::new_sync(workspace.clone());
        let mut tools = ToolRegistry::new();

        // Register filesystem tools
        let allowed_dir = if config.restrict_to_workspace {
            Some(workspace.clone())
        } else {
            None
        };

        tools.register(Box::new(ReadFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(WriteFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(EditFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(ListDirTool::new(allowed_dir)));

        // Register shell tool
        tools.register(Box::new(ExecTool::new(
            workspace.clone(),
            std::time::Duration::from_secs(120),
            config.restrict_to_workspace,
        )));

        // Register web tools
        tools.register(Box::new(WebFetchTool::new()));
        tools.register(Box::new(WebSearchTool::new(deps.web_tools)));

        // Register spawn tool
        tools.register(Box::new(SpawnTool::new()));

        // Register message tool (requires bus)
        if let Some(bus) = &deps.bus {
            tools.register(Box::new(MessageTool::new(bus.clone())));
        }

        // Register cron tool (requires cron service)
        if let Some(cron_service) = &deps.cron_service {
            tools.register(Box::new(CronTool::new(cron_service.clone())));
        }

        // Register MCP tool bridges
        for mcp_tool in deps.mcp_tools {
            tools.register(mcp_tool);
        }

        // Load skills
        let skills_context = Self::load_skills(&workspace);

        // Build context with skills
        let context = ContextBuilder::new(workspace.clone())?.with_skills_context(skills_context);

        // Build tool middleware stack
        let mut tool_middleware = MiddlewareStack::new();
        // Always add logging as the outermost layer
        tool_middleware.push(Arc::new(ToolLoggingMiddleware));
        // Add user-provided middlewares
        for mw in deps.tool_middleware {
            tool_middleware.push(mw);
        }

        Ok(Self {
            provider,
            context,
            memory,
            sessions,
            tools,
            config,
            workspace,
            tool_middleware,
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
    pub async fn process_direct(&self, content: &str, session_key: &str) -> Result<String> {
        let mut session = self.sessions.get_or_create(session_key).await;

        // Handle slash commands
        let cmd = content.trim().to_lowercase();
        if cmd == "/new" {
            session.clear();
            self.sessions.save(&session).await;
            return Ok("New session started.".to_string());
        }
        if cmd == "/help" {
            return Ok("🐈 nanobot commands:\n/new — Start a new conversation\n/help — Show available commands".to_string());
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
        let (response, _tools_used) = self.run_agent_loop(messages).await?;

        // Save to session
        session.add_message("user", content, None);
        session.add_message("assistant", &response, None);
        self.sessions.save(&session).await;

        Ok(response)
    }

    /// Run the agent iteration loop
    async fn run_agent_loop(
        &self,
        initial_messages: Vec<ChatMessage>,
    ) -> Result<(String, Vec<String>)> {
        let mut messages = initial_messages;
        let mut iteration = 0;
        let mut final_content = None;
        let mut tools_used = Vec::new();
        let executor = ToolExecutor::new(
            &self.tools,
            self.config.max_tool_result_chars,
            &self.tool_middleware,
        );

        // Create a trail context for this agent run
        let trail_ctx = TrailContext::new();

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
            };

            let mut retries = 0;
            let max_retries = 3;
            let response = loop {
                match self.provider.chat(request.clone(), &trail_ctx).await {
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
                    last.tool_calls = Some(std::mem::take(&mut { response.tool_calls.clone() }));
                    // Minor optimization
                }

                // Execute each tool call via ToolExecutor
                let results = executor.execute_batch(&response.tool_calls).await;
                for result in results {
                    tools_used.push(result.tool_name.clone());
                    self.context.add_tool_result(
                        &mut messages,
                        result.tool_call_id,
                        result.tool_name,
                        result.output,
                    );
                }

                // No reflection message — the LLM already has the tool results
                // and will decide next steps on its own.
            } else {
                final_content = response.content;
                break;
            }
        }

        let content = final_content.unwrap_or_else(|| {
            "I've completed processing but have no response to give.".to_string()
        });

        Ok((content, tools_used))
    }
}
