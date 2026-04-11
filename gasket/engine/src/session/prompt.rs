//! Prompt loading utilities.
//!
//! Provides functions to load workspace bootstrap files and skills context
//! for injection into the system prompt. These are called directly by
//! `AgentLoop` during initialization — no dynamic hook dispatch needed.

use std::path::Path;

use tokio::fs;
use tracing::{debug, info, warn};

use gasket_storage::count_tokens;

/// Bootstrap files loaded into the system prompt for the full (main agent) profile
pub const BOOTSTRAP_FILES_FULL: &[&str] = &[
    "PROFILE.md",
    "SOUL.md",
    "AGENTS.md",
    "MEMORY.md",
    "BOOTSTRAP.md",
];

/// Bootstrap files loaded for the minimal (subagent) profile — only core identity
pub const BOOTSTRAP_FILES_MINIMAL: &[&str] = &["PROFILE.md", "SOUL.md"];

/// Maximum tokens allowed per single bootstrap file before emitting a warning
const BOOTSTRAP_TOKEN_WARN_THRESHOLD: usize = 2000;

/// Hard token limit for MEMORY.md — content exceeding this is truncated.
/// Keeps the tail (most recent entries) and drops the head (oldest entries).
pub const MEMORY_TOKEN_HARD_LIMIT: usize = 2048;

/// Files subject to hard token truncation (not just a warning).
const TRUNCATABLE_FILES: &[&str] = &["MEMORY.md"];

/// Load the system prompt from workspace bootstrap files.
///
/// Reads the specified files from the workspace directory and concatenates them.
/// Returns an identity header plus any loaded bootstrap file contents.
/// If no files are found, returns only the identity header.
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
        "你是乐子🐈, 我的personal AI assistant.\nYour working directory: {}.\n YOU can ONLY READ and WRITE under working directory.",
        workspace.display()
    ));

    // Load bootstrap files
    let mut loaded_any = false;
    let mut total_tokens: usize = 0;
    for filename in files {
        let file_path = workspace.join(filename);
        if file_path.exists() {
            let raw_content = fs::read_to_string(&file_path).await?;
            if !raw_content.trim().is_empty() {
                let trimmed = raw_content.trim();
                let tokens = count_tokens(trimmed);

                // Hard truncation for memory-class files (e.g. MEMORY.md)
                let content =
                    if TRUNCATABLE_FILES.contains(filename) && tokens > MEMORY_TOKEN_HARD_LIMIT {
                        warn!(
                        "Bootstrap file {} has {} tokens (hard limit {}). Truncating tail-keep.",
                        filename, tokens, MEMORY_TOKEN_HARD_LIMIT
                    );
                        truncate_keep_tail(trimmed, MEMORY_TOKEN_HARD_LIMIT)
                    } else {
                        if tokens > BOOTSTRAP_TOKEN_WARN_THRESHOLD {
                            warn!(
                            "Bootstrap file {} has {} tokens (threshold {}). Consider trimming it.",
                            filename, tokens, BOOTSTRAP_TOKEN_WARN_THRESHOLD
                        );
                        }
                        trimmed.to_string()
                    };

                let final_tokens = count_tokens(&content);
                total_tokens += final_tokens;
                debug!(
                    "Loaded bootstrap file: {} ({} tokens{})",
                    filename,
                    final_tokens,
                    if final_tokens < tokens {
                        format!(", truncated from {}", tokens)
                    } else {
                        String::new()
                    }
                );
                parts.push(format!("## {}\n\n{}", filename, content));
                loaded_any = true;
            }
        }
    }

    info!(
        "System prompt: {} bootstrap files loaded ({} found), ~{} tokens total",
        files.len(),
        loaded_any,
        total_tokens
    );

    Ok(parts.join("\n\n"))
}

/// Load the skills context from the workspace.
///
/// Scans for skill definitions and returns a formatted string for prompt injection,
/// or `None` if no skills are found.
pub async fn load_skills_context(workspace: &Path) -> Option<String> {
    let ctx = super::load_skills(workspace).await?;
    if ctx.is_empty() {
        None
    } else {
        Some(format!("# Skills\n\n{}", ctx))
    }
}

/// Truncate content to fit within `max_tokens`, keeping the **tail** (most recent
/// entries) and dropping lines from the head. Prepends a system warning so the
/// agent knows it must clean up.
pub fn truncate_keep_tail(content: &str, max_tokens: usize) -> String {
    let warning = "[SYSTEM WARNING: This file was truncated because it exceeded the token limit. \
        Oldest entries were removed. Use 'read_file' to view the full file on disk, \
        and 'edit_file' to prune, summarize, or move details to separate files in memory/.]";
    let warning_tokens = count_tokens(warning) + 10; // margin for newlines
    let budget = max_tokens.saturating_sub(warning_tokens);

    let lines: Vec<&str> = content.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut kept_tokens: usize = 0;

    // Walk from the tail (newest) toward the head (oldest)
    for line in lines.iter().rev() {
        let line_tokens = count_tokens(line);
        if kept_tokens + line_tokens > budget {
            break;
        }
        kept.push(line);
        kept_tokens += line_tokens;
    }

    kept.reverse();
    format!("{}\n\n{}", warning, kept.join("\n"))
}
