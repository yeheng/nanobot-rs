//! Prompt loading utilities.
//!
//! Provides functions to load workspace bootstrap files and skills context
//! for injection into the system prompt. These are called directly by
//! `AgentLoop` during initialization — no dynamic hook dispatch needed.

use std::path::Path;

use tokio::fs;
use tracing::{debug, info, warn};

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

/// Load the system prompt from workspace bootstrap files.
///
/// Reads the specified files from the workspace directory, concatenates them,
/// and prepends an identity header. If no files are found, falls back to
/// default instructions.
///
/// # Errors
/// Returns an error if a bootstrap file **exists** but cannot be read.
pub async fn load_system_prompt(
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

    Ok(parts.join("\n\n"))
}

/// Load the skills context from the workspace.
///
/// Scans for skill definitions and returns a formatted string for prompt injection,
/// or `None` if no skills are found.
pub async fn load_skills_context(workspace: &Path) -> Option<String> {
    let ctx = skill_loader::load_skills(workspace).await?;
    if ctx.is_empty() {
        None
    } else {
        Some(format!("# Skills\n\n{}", ctx))
    }
}
