//! Context builder for constructing LLM prompts

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, warn};

use crate::memory::SqliteStore;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use crate::session::SessionMessage;

use super::history_processor::{count_tokens, process_history, HistoryConfig};

/// Bootstrap files loaded into the system prompt (same as Python version)
const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];

/// Fixed prompt for LLM summarization
const SUMMARIZATION_PROMPT: &str = "Summarize the following conversation briefly, keeping key facts.";

/// Prefix for injected summary assistant messages
const SUMMARY_PREFIX: &str = "[Conversation Summary]: ";

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
    /// LLM provider for summarization calls
    provider: Option<Arc<dyn LlmProvider>>,
    /// SQLite store for summary persistence
    store: Option<Arc<SqliteStore>>,
    /// Model name for summarization requests
    model: Option<String>,
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

        Ok(Self {
            _workspace: workspace,
            system_prompt: Arc::new(system_prompt),
            skills_context: None,
            history_config,
            provider: None,
            store: None,
            model: None,
        })
    }

    /// Create a context builder with custom history configuration
    pub fn with_history_config(mut self, config: HistoryConfig) -> Self {
        self.history_config = config;
        self
    }

    /// Create a context builder with "smart" history processing
    /// (token budget management)
    pub fn with_smart_history(mut self, token_budget: usize) -> Self {
        self.history_config.token_budget = token_budget;
        self
    }

    /// Set the LLM provider and SQLite store for summarization support.
    pub fn with_summarization(
        mut self,
        provider: Arc<dyn LlmProvider>,
        store: Arc<SqliteStore>,
        model: String,
    ) -> Self {
        self.provider = Some(provider);
        self.store = Some(store);
        self.model = Some(model);
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
    /// If summarization is configured (provider + store), this method will:
    /// 1. Load any existing summary from SQLite
    /// 2. Check if history exceeds token/message budgets
    /// 3. If so, call the LLM to summarize older messages
    /// 4. Persist the summary and clean up old messages
    /// 5. Inject the summary as an assistant message
    pub async fn build_messages(
        &self,
        history: Vec<SessionMessage>,
        _current_message: &str,
        memory: Option<&str>,
        _channel: &str,
        _chat_id: &str,
        session_key: &str,
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

        // Load existing summary (if store is configured)
        let existing_summary = if let Some(store) = &self.store {
            match store.load_session_summary(session_key).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to load session summary: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Process history with token budget awareness
        let processed = process_history(history, &self.history_config);

        // Store stats before moving messages
        let history_count = processed.messages.len();
        let filtered_count = processed.filtered_count;
        let estimated_tokens = processed.estimated_tokens;

        // Check if summarization is needed and configured
        let needs_summarization = filtered_count > 0
            && self.provider.is_some()
            && self.store.is_some()
            && self.model.is_some();

        let summary = if needs_summarization {
            // We had messages that were filtered out — summarize them
            // The filtered messages are the ones that process_history dropped.
            // We need to summarize whatever we have so far (existing summary + what was dropped).
            match self.run_summarization(session_key, &processed.messages, &existing_summary).await
            {
                Ok(new_summary) => Some(new_summary),
                Err(e) => {
                    warn!("Summarization failed, using existing summary as fallback: {}", e);
                    existing_summary
                }
            }
        } else {
            existing_summary
        };

        // Inject summary as assistant message (if exists)
        if let Some(ref summary_text) = summary {
            if !summary_text.is_empty() {
                messages.push(ChatMessage::assistant(format!(
                    "{}{}",
                    SUMMARY_PREFIX, summary_text
                )));
            }
        }

        // Add processed history messages
        for msg in processed.messages {
            match msg.role.as_str() {
                "user" => messages.push(ChatMessage::user(&msg.content)),
                "assistant" => messages.push(ChatMessage::assistant(&msg.content)),
                _ => {}
            }
        }

        // Current message
        messages.push(ChatMessage::user(_current_message));

        debug!(
            "Built messages: {} history ({} filtered, {} tokens est.), summary: {}",
            history_count,
            filtered_count,
            estimated_tokens,
            summary.is_some()
        );

        messages
    }

    /// Run LLM summarization for older messages.
    ///
    /// Builds a summarization prompt from existing summary + recent messages,
    /// calls the provider, and persists the result.
    async fn run_summarization(
        &self,
        session_key: &str,
        recent_messages: &[SessionMessage],
        existing_summary: &Option<String>,
    ) -> anyhow::Result<String> {
        let provider = self.provider.as_ref().unwrap();
        let store = self.store.as_ref().unwrap();
        let model = self.model.as_ref().unwrap();

        // Build context for summarization: existing summary + recent messages
        let mut context_parts = Vec::new();
        if let Some(existing) = existing_summary {
            if !existing.is_empty() {
                context_parts.push(format!("Previous summary:\n{}", existing));
            }
        }

        // Include recent messages as context for summarization
        for msg in recent_messages {
            context_parts.push(format!("{}: {}", msg.role, msg.content));
        }

        let context_text = context_parts.join("\n");

        // Count tokens of context to avoid sending too much
        let context_tokens = count_tokens(&context_text);
        debug!(
            "Summarization context: {} tokens, {} messages",
            context_tokens,
            recent_messages.len()
        );

        // Build the summarization request
        let summarization_messages = vec![
            ChatMessage::system(SUMMARIZATION_PROMPT),
            ChatMessage::user(context_text),
        ];

        let request = ChatRequest {
            model: model.clone(),
            messages: summarization_messages,
            tools: None,
            temperature: Some(0.3), // Low temperature for factual summarization
            max_tokens: Some(1024),
            thinking: None,
        };

        let response = provider.chat(request).await?;
        let summary_text = response
            .content
            .unwrap_or_default()
            .trim()
            .to_string();

        if summary_text.is_empty() {
            anyhow::bail!("Summarization returned empty content");
        }

        // Persist the summary
        store
            .save_session_summary(session_key, &summary_text)
            .await?;

        debug!(
            "Generated and saved session summary for {}: {} tokens",
            session_key,
            count_tokens(&summary_text)
        );

        Ok(summary_text)
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
