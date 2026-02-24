//! Context builder for constructing LLM prompts

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::debug;

use crate::providers::ChatMessage;
use crate::session::SessionMessage;

use super::history_processor::{HistoryConfig, HistoryStrategy, StrategyFactory};

/// Bootstrap files loaded into the system prompt (same as Python version)
const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];

/// Context builder for constructing prompts.
///
/// This struct is designed to be created once at startup and shared across
/// multiple agent loops via `Arc`. The system prompt is built once and cached
/// to avoid repeated synchronous file I/O in async contexts.
#[derive(Clone)]
pub struct ContextBuilder {
    _workspace: PathBuf,
    system_prompt: Arc<String>,
    skills_context: Option<Arc<String>>,
    /// History processing configuration
    history_config: HistoryConfig,
    /// History processing strategy (boxed trait object)
    history_strategy: Arc<dyn HistoryStrategy>,
}

impl ContextBuilder {
    /// Create a new context builder.
    ///
    /// Loads bootstrap files (AGENTS.md, SOUL.md, USER.md, TOOLS.md) from the
    /// workspace directory. Falls back to a minimal default prompt if none exist.
    ///
    /// # Errors
    ///
    /// Returns an error if a bootstrap file **exists** but cannot be read
    /// (permission denied, I/O error, etc.). A missing file is not an error.
    ///
    /// # Note
    ///
    /// This constructor performs synchronous file I/O. It should be called
    /// during startup, not in async contexts. For subagents, use the cached
    /// instance from the parent agent.
    pub fn new(workspace: PathBuf) -> Result<Self, std::io::Error> {
        let system_prompt = Self::build_system_prompt(&workspace)?;
        let history_config = HistoryConfig::default();
        let factory = StrategyFactory::new(history_config.clone());

        Ok(Self {
            _workspace: workspace,
            system_prompt: Arc::new(system_prompt),
            skills_context: None,
            history_config,
            // Use smart strategy by default: relevance filtering + token budget
            history_strategy: Arc::from(factory.create_smart()),
        })
    }

    /// Create a context builder with custom history configuration
    pub fn with_history_config(mut self, config: HistoryConfig) -> Self {
        self.history_config = config.clone();
        let factory = StrategyFactory::new(config);
        // Use smart strategy by default for better context management
        self.history_strategy = Arc::from(factory.create_smart());
        self
    }

    /// Create a context builder with a custom history strategy
    pub fn with_history_strategy(mut self, strategy: Arc<dyn HistoryStrategy>) -> Self {
        self.history_strategy = strategy;
        self
    }

    /// Create a context builder with "smart" history processing
    /// (relevance filtering + token budget)
    pub fn with_smart_history(mut self, token_budget: usize) -> Self {
        self.history_config.token_budget = token_budget;
        let factory = StrategyFactory::new(self.history_config.clone());
        self.history_strategy = Arc::from(factory.create_smart());
        self
    }

    /// Build system prompt from workspace bootstrap files.
    ///
    /// Files that don't exist are silently skipped. Files that exist but fail
    /// to read cause an immediate error — silent degradation on core config is
    /// dangerous.
    fn build_system_prompt(workspace: &Path) -> Result<String, std::io::Error> {
        let mut parts = Vec::new();

        // Identity header
        parts.push(format!(
            "You are nanobot 🐈, a personal AI assistant.\n\nWorking directory: {}",
            workspace.display()
        ));

        // Load bootstrap files
        let mut loaded_any = false;
        for filename in BOOTSTRAP_FILES {
            let file_path = workspace.join(filename);
            if file_path.exists() {
                // File exists — a read failure here is a hard error.
                let content = std::fs::read_to_string(&file_path)?;
                if !content.trim().is_empty() {
                    debug!("Loaded bootstrap file: {}", filename);
                    parts.push(format!("## {}\n\n{}", filename, content.trim()));
                    loaded_any = true;
                }
            }
        }

        if !loaded_any {
            // Fallback: use minimal default instructions
            parts.push(DEFAULT_INSTRUCTIONS.to_string());
        }

        Ok(parts.join("\n\n"))
    }

    /// Set a custom system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Arc::new(prompt.into());
        self
    }

    /// Set skills context summary
    pub fn with_skills_context(mut self, context: Option<String>) -> Self {
        self.skills_context = context.map(Arc::new);
        self
    }

    /// Get a cloneable reference to the context builder.
    /// Useful for sharing with subagents.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Build the message list for an LLM request.
    ///
    /// Uses the configured history strategy to process conversation history.
    /// The strategy may truncate, filter, summarize, or inject history based
    /// on configuration and current input relevance.
    pub fn build_messages(
        &self,
        history: Vec<SessionMessage>,
        current_message: &str,
        memory: Option<&str>,
        _channel: &str,
        _chat_id: &str,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System prompt
        let mut system_content = (*self.system_prompt).clone();
        if let Some(mem) = memory {
            if !mem.is_empty() {
                system_content.push_str("\n\n## Long-term Memory\n");
                system_content.push_str(mem);
            }
        }
        if let Some(skills) = &self.skills_context {
            if !skills.is_empty() {
                system_content.push_str("\n\n# Skills\n\n");
                system_content.push_str(skills);
            }
        }
        messages.push(ChatMessage::system(system_content));

        // Process history using the configured strategy
        let processed =
            self.history_strategy
                .process(history, current_message, &self.history_config);

        // Store stats before moving messages
        let history_count = processed.messages.len();
        let filtered_count = processed.filtered_count;
        let estimated_tokens = processed.estimated_tokens;

        // Add processed history messages
        for msg in processed.messages {
            match msg.role.as_str() {
                "user" => messages.push(ChatMessage::user(&msg.content)),
                "assistant" => messages.push(ChatMessage::assistant(&msg.content)),
                _ => {}
            }
        }

        // Current message
        messages.push(ChatMessage::user(current_message));

        debug!(
            "Built messages: {} history ({} filtered, {} tokens est.)",
            history_count, filtered_count, estimated_tokens
        );

        messages
    }

    /// Add an assistant message to the history
    pub fn add_assistant_message(
        &self,
        messages: &mut Vec<ChatMessage>,
        content: Option<String>,
        _tool_calls: Vec<serde_json::Value>,
        _reasoning_content: Option<String>,
    ) {
        if let Some(c) = content {
            messages.push(ChatMessage::assistant(c));
        }
    }

    /// Add a tool result to the messages
    pub fn add_tool_result(
        &self,
        messages: &mut Vec<ChatMessage>,
        tool_id: String,
        tool_name: String,
        result: String,
    ) {
        messages.push(ChatMessage::tool_result(tool_id, tool_name, result));
    }
}

/// Fallback instructions when no bootstrap files exist
const DEFAULT_INSTRUCTIONS: &str = r#"You have access to tools for reading files, writing files, editing files, listing directories, and executing shell commands.

Be concise and helpful. When using tools, explain what you're doing before and after the tool call."#;
