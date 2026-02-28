//! Context builder for constructing LLM prompts

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;
use tracing::{debug, info, warn};

use crate::memory::SqliteStore;
use crate::providers::{ChatMessage, LlmProvider};
use crate::session::SessionMessage;

use super::history_processor::{count_tokens, process_history, HistoryConfig};
use super::summarization::{SummarizationService, SUMMARY_PREFIX};

/// Bootstrap files loaded into the system prompt for the full (main agent) profile
const BOOTSTRAP_FILES_FULL: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];

/// Bootstrap files loaded for the minimal (subagent) profile — only core identity
const BOOTSTRAP_FILES_MINIMAL: &[&str] = &["SOUL.md"];

/// Maximum tokens allowed per single bootstrap file before emitting a warning
const BOOTSTRAP_TOKEN_WARN_THRESHOLD: usize = 2000;

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
    pub async fn new(workspace: PathBuf) -> Result<Self, std::io::Error> {
        let system_prompt = Self::build_system_prompt(&workspace, BOOTSTRAP_FILES_FULL).await?;
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

    /// Create a minimal context builder for subagents.
    ///
    /// Only loads SOUL.md (core identity) and skips skills context to save tokens.
    /// Subagents execute focused background tasks and don't need the full prompt.
    pub async fn new_minimal(workspace: PathBuf) -> Result<Self, std::io::Error> {
        let system_prompt = Self::build_system_prompt(&workspace, BOOTSTRAP_FILES_MINIMAL).await?;
        let history_config = HistoryConfig {
            max_messages: 20,
            token_budget: 4000,
            recent_keep: 5,
        };

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

    /// Derive a minimal context builder from an existing (full) instance.
    ///
    /// Rebuilds the system prompt with only SOUL.md and drops skills context.
    /// This is the recommended way to create subagent contexts after startup.
    pub async fn to_minimal(&self) -> Result<Self, std::io::Error> {
        let system_prompt =
            Self::build_system_prompt(&self._workspace, BOOTSTRAP_FILES_MINIMAL).await?;

        Ok(Self {
            _workspace: self._workspace.clone(),
            system_prompt: Arc::new(system_prompt),
            skills_context: None,
            history_config: HistoryConfig {
                max_messages: 20,
                token_budget: 4000,
                recent_keep: 5,
            },
            provider: self.provider.clone(),
            store: self.store.clone(),
            model: self.model.clone(),
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
    /// `files` controls which bootstrap files are loaded — pass
    /// `BOOTSTRAP_FILES_FULL` for the main agent or `BOOTSTRAP_FILES_MINIMAL`
    /// for subagents.
    ///
    /// Files that don't exist are silently skipped. Files that exist but fail
    /// to read cause an immediate error — silent degradation on core config is
    /// dangerous.
    ///
    /// A warning is logged for any file exceeding `BOOTSTRAP_TOKEN_WARN_THRESHOLD`.
    async fn build_system_prompt(
        workspace: &Path,
        files: &[&str],
    ) -> Result<String, std::io::Error> {
        let mut parts = Vec::new();

        // Identity header
        parts.push(format!(
            "你叫阿乐 🐈, 夜痕的专业私人助理.\n\nWorking directory: {}",
            workspace.display()
        ));

        // Load bootstrap files
        let mut loaded_any = false;
        let mut total_tokens: usize = 0;
        for filename in files {
            let file_path = workspace.join(filename);
            if file_path.exists() {
                // File exists — a read failure here is a hard error.
                let content = fs::read_to_string(&file_path).await?;
                if !content.trim().is_empty() {
                    let tokens = count_tokens(content.trim());
                    if tokens > BOOTSTRAP_TOKEN_WARN_THRESHOLD {
                        warn!(
                            "Bootstrap file {} has {} tokens (threshold {}). Consider trimming it.",
                            filename, tokens, BOOTSTRAP_TOKEN_WARN_THRESHOLD
                        );
                    }
                    total_tokens += tokens;
                    debug!("Loaded bootstrap file: {} ({} tokens)", filename, tokens);
                    parts.push(format!("## {}\n\n{}", filename, content.trim()));
                    loaded_any = true;
                }
            }
        }

        if !loaded_any {
            // Fallback: use minimal default instructions
            parts.push(DEFAULT_INSTRUCTIONS.to_string());
        }

        info!(
            "System prompt: {} bootstrap files, ~{} tokens total",
            files.len(),
            total_tokens
        );

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

        // Process history with token budget awareness
        let processed = process_history(history, &self.history_config);

        // Store stats before moving messages
        let history_count = processed.messages.len();
        let filtered_count = processed.filtered_count;
        let estimated_tokens = processed.estimated_tokens;

        // Check if summarization is needed and configured
        // Only summarize if we have evicted messages (old messages that exceeded budget)
        let summary = if !processed.evicted.is_empty()
            && self.provider.is_some()
            && self.store.is_some()
            && self.model.is_some()
        {
            // Create summarization service and run summarization
            let service = SummarizationService::new(
                self.provider.as_ref().unwrap().clone(),
                self.store.as_ref().unwrap().clone(),
                self.model.as_ref().unwrap().clone(),
            );

            let existing_summary = service.load_summary(session_key).await;

            // Summarize the EVICTED messages (old messages that were dropped from context)
            match service
                .summarize(session_key, &processed.evicted, &existing_summary)
                .await
            {
                Ok(new_summary) => Some(new_summary),
                Err(e) => {
                    warn!(
                        "Summarization failed, using existing summary as fallback: {}",
                        e
                    );
                    existing_summary
                }
            }
        } else if self.provider.is_some() && self.store.is_some() && self.model.is_some() {
            // Load existing summary if store is configured but no summarization needed
            let service = SummarizationService::new(
                self.provider.as_ref().unwrap().clone(),
                self.store.as_ref().unwrap().clone(),
                self.model.as_ref().unwrap().clone(),
            );
            service.load_summary(session_key).await
        } else {
            None
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

    /// Add an assistant message to the history
    pub fn add_assistant_message(
        &self,
        messages: &mut Vec<ChatMessage>,
        content: Option<String>,
        tool_calls: Vec<crate::providers::ToolCall>,
    ) {
        if tool_calls.is_empty() {
            // No tool calls - simple assistant message
            if let Some(c) = content {
                messages.push(ChatMessage::assistant(c));
            }
        } else {
            // Has tool calls - must include them in the message
            messages.push(ChatMessage::assistant_with_tools(content, tool_calls));
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
