//! Prompt loading hooks.
//!
//! Provides `BootstrapHook` and `SkillsHook` to inject static context
//! into the system prompt during the `on_context_prepare` phase,
//! eliminating the need for `ContextBuilder`.

use std::path::Path;

use tokio::fs;
use tracing::{debug, info, warn};

use super::{AgentHook, ContextPrepareContext};
use crate::agent::history_processor::count_tokens;
use crate::agent::skill_loader;

/// Bootstrap files loaded into the system prompt for the full (main agent) profile
pub const BOOTSTRAP_FILES_FULL: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];

/// Bootstrap files loaded for the minimal (subagent) profile — only core identity
pub const BOOTSTRAP_FILES_MINIMAL: &[&str] = &["SOUL.md"];

/// Maximum tokens allowed per single bootstrap file before emitting a warning
const BOOTSTRAP_TOKEN_WARN_THRESHOLD: usize = 2000;

/// Fallback instructions when no bootstrap files exist
const DEFAULT_INSTRUCTIONS: &str = r#"You have access to tools for reading files, writing files, editing files, listing directories, and executing shell commands.

Be concise and helpful. When using tools, explain what you're doing before and after the tool call."#;

// ── BootstrapHook ───────────────────────────────────────────

/// Hook for loading workspace bootstrap files (`AGENTS.md`, `SOUL.md`, etc.).
///
/// Loads the files exactly once at agent creation time, and injects them
/// into `ContextPrepareContext::system_prompts` on every request.
pub struct BootstrapHook {
    prompt: String,
}

impl BootstrapHook {
    /// Create a new `BootstrapHook`, loading the specified files.
    ///
    /// # Errors
    /// Returns an error if a bootstrap file **exists** but cannot be read.
    pub async fn new(workspace: &Path, files: &[&str]) -> Result<Self, std::io::Error> {
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
            parts.push(DEFAULT_INSTRUCTIONS.to_string());
        }

        info!(
            "System prompt: {} bootstrap files, ~{} tokens total",
            files.len(),
            total_tokens
        );

        Ok(Self {
            prompt: parts.join("\n\n"),
        })
    }

    /// Create for the main agent (all default files).
    pub async fn new_full(workspace: &Path) -> Result<Self, std::io::Error> {
        Self::new(workspace, BOOTSTRAP_FILES_FULL).await
    }

    /// Create for a subagent (minimal identity only).
    pub async fn new_minimal(workspace: &Path) -> Result<Self, std::io::Error> {
        Self::new(workspace, BOOTSTRAP_FILES_MINIMAL).await
    }
}

#[async_trait::async_trait]
impl AgentHook for BootstrapHook {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn on_context_prepare(&self, ctx: &mut ContextPrepareContext) {
        ctx.system_prompts.push(self.prompt.clone());
    }
}

// ── SkillsHook ──────────────────────────────────────────────

/// Hook for injecting available skills into the system prompt.
///
/// Discovers and parses skills exactly once at agent creation time.
pub struct SkillsHook {
    skills_context: Option<String>,
}

impl SkillsHook {
    /// Create a new `SkillsHook`, scanning the workspace for skills.
    pub async fn new(workspace: &Path) -> Self {
        let skills_ctx = skill_loader::load_skills(workspace).await;
        Self {
            skills_context: skills_ctx,
        }
    }
}

#[async_trait::async_trait]
impl AgentHook for SkillsHook {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn on_context_prepare(&self, ctx: &mut ContextPrepareContext) {
        if let Some(ref skills) = self.skills_context {
            if !skills.is_empty() {
                ctx.system_prompts.push(format!("# Skills\n\n{}", skills));
            }
        }
    }
}
