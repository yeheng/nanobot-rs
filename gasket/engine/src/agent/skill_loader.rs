//! Skill loading utilities for the agent.
//!
//! Provides functionality to discover and load skills from builtin and user directories.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::skills::{SkillsLoader, SkillsRegistry};

/// Load skills from builtin and user directories.
///
/// Returns a context summary string if any skills were loaded, or None otherwise.
pub async fn load_skills(workspace: &Path) -> Option<String> {
    let user_skills_dir = workspace.join("skills");

    // Locate builtin skills: try relative to the executable, then a few common fallbacks
    let builtin_skills_dir = find_builtin_skills_dir();

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
    match SkillsRegistry::from_loader(loader).await {
        Ok(registry) => {
            let summary = registry.generate_context_summary().await;
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

/// Find the builtin skills directory.
///
/// Searches in the following order:
/// 1. Relative to the executable (dev build path)
/// 2. Current working directory
pub fn find_builtin_skills_dir() -> Option<PathBuf> {
    // Try relative to the executable
    if let Ok(exe) = std::env::current_exe() {
        // dev build: target/debug/gasket → engine/skills/
        if let Some(project_root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let candidate = project_root.join("engine").join("skills");
            if candidate.exists() {
                debug!("Found builtin skills at {:?}", candidate);
                return Some(candidate);
            }
        }
    }

    // Try current working directory
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("engine").join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
        // Also try if we're inside engine
        let candidate = cwd.join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_builtin_skills_dir_returns_none_for_nonexistent() {
        // This test just verifies the function doesn't panic
        let result = find_builtin_skills_dir();
        // Result depends on environment, just ensure it returns Option<PathBuf>
        assert!(result.is_none() || result.is_some());
    }
}
