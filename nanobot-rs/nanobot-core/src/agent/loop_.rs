//! Agent loop: the core processing engine

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, instrument};

use crate::agent::context::ContextBuilder;
use crate::agent::memory::MemoryStore;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use crate::session::SessionManager;
use crate::tools::{
    EditFileTool, ExecTool, ListDirTool, ReadFileTool, ToolRegistry, WriteFileTool,
};

/// Agent loop configuration
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub memory_window: usize,
    pub restrict_to_workspace: bool,
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
}

impl AgentLoop {
    /// Create a new agent loop
    pub fn new(provider: Arc<dyn LlmProvider>, workspace: PathBuf, config: AgentConfig) -> Self {
        let context = ContextBuilder::new(workspace.clone());
        let memory = MemoryStore::new(workspace.clone());
        let sessions = SessionManager::new(workspace.clone());
        let mut tools = ToolRegistry::new();

        // Register default tools
        let allowed_dir = if config.restrict_to_workspace {
            Some(workspace.clone())
        } else {
            None
        };

        tools.register(Box::new(ReadFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(WriteFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(EditFileTool::new(allowed_dir.clone())));
        tools.register(Box::new(ListDirTool::new(allowed_dir)));
        tools.register(Box::new(ExecTool::new(
            workspace.clone(),
            std::time::Duration::from_secs(120),
            config.restrict_to_workspace,
        )));

        Self {
            provider,
            context,
            memory,
            sessions,
            tools,
            config,
            workspace,
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
        let memory_content = self.memory.read_long_term().ok();
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

        while iteration < self.config.max_iterations {
            iteration += 1;
            debug!("Agent loop iteration {}", iteration);

            let request = ChatRequest {
                model: self.config.model.clone(),
                messages: messages.clone(),
                tools: Some(self.tools.get_definitions()),
                temperature: Some(self.config.temperature),
                max_tokens: Some(self.config.max_tokens),
            };

            let response = self.provider.chat(request).await?;

            if response.has_tool_calls {
                // Add assistant message with tool calls
                messages.push(ChatMessage::assistant_with_tools(
                    response.content.clone(),
                    response.tool_calls.clone(),
                ));

                // Execute each tool call
                for tool_call in &response.tool_calls {
                    tools_used.push(tool_call.function.name.clone());
                    info!(
                        "Tool call: {}({:?})",
                        tool_call.function.name, tool_call.function.arguments
                    );

                    let result = self
                        .tools
                        .execute(
                            &tool_call.function.name,
                            tool_call.function.arguments.clone(),
                        )
                        .await;

                    let result_str = match result {
                        Ok(r) => r,
                        Err(e) => format!("Error: {}", e),
                    };

                    messages.push(ChatMessage::tool_result(
                        &tool_call.id,
                        &tool_call.function.name,
                        result_str,
                    ));
                }

                // Add a user message to prompt continuation
                messages.push(ChatMessage::user(
                    "Reflect on the results and decide next steps.",
                ));
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
