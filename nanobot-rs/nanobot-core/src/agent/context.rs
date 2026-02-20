//! Context builder for constructing LLM prompts

use std::path::PathBuf;

use tracing::debug;

use crate::providers::ChatMessage;
use crate::session::SessionMessage;

/// Bootstrap files loaded into the system prompt (same as Python version)
const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];

/// Context builder for constructing prompts
pub struct ContextBuilder {
    workspace: PathBuf,
    system_prompt: String,
    skills_context: Option<String>,
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
    pub fn new(workspace: PathBuf) -> Result<Self, std::io::Error> {
        let system_prompt = Self::build_system_prompt(&workspace)?;
        Ok(Self {
            workspace,
            system_prompt,
            skills_context: None,
        })
    }

    /// Build system prompt from workspace bootstrap files.
    ///
    /// Files that don't exist are silently skipped. Files that exist but fail
    /// to read cause an immediate error — silent degradation on core config is
    /// dangerous.
    fn build_system_prompt(workspace: &PathBuf) -> Result<String, std::io::Error> {
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
        self.system_prompt = prompt.into();
        self
    }

    /// Set skills context summary
    pub fn with_skills_context(mut self, context: Option<String>) -> Self {
        self.skills_context = context;
        self
    }

    /// Build the message list for an LLM request.
    ///
    /// When the history exceeds `recent_window` messages, older messages are
    /// condensed to save tokens: only the first 100 characters of each old
    /// message are kept, prefixed with its role.
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
        let mut system_content = self.system_prompt.clone();
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

        // History — apply progressive trimming.
        // Keep the most recent RECENT_KEEP messages verbatim; condense older ones.
        const RECENT_KEEP: usize = 10;
        let total = history.len();
        let trim_boundary = total.saturating_sub(RECENT_KEEP);

        for (i, msg) in history.iter().enumerate() {
            let content = if i < trim_boundary {
                // Condense: keep only first 100 chars
                truncate_content(&msg.content, 100)
            } else {
                msg.content.clone()
            };

            match msg.role.as_str() {
                "user" => messages.push(ChatMessage::user(&content)),
                "assistant" => messages.push(ChatMessage::assistant(&content)),
                _ => {}
            }
        }

        // Current message
        messages.push(ChatMessage::user(current_message));

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

/// Truncate text to `max_chars`, appending "..." if shortened.
fn truncate_content(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Find a safe char boundary
    let mut end = max_chars;
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &text[..end])
}
