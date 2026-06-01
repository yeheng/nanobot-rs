//! Skill loading utilities — resolved builtin paths and workspace discovery.

use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::skills::{SkillsLoader, SkillsRegistry};

/// Load skills from builtin and user directories, plus skill-mode workflows.
///
/// Returns a context summary string if any skills were loaded, or None otherwise.
pub async fn load_skills(workspace: &Path) -> Option<String> {
    let user_skills_dir = workspace.join("skills");
    let builtin_skills_dir = find_builtin_skills_dir();

    if builtin_skills_dir.is_none() {
        debug!("Built-in skills directory not found, loading user skills only");
        if !user_skills_dir.exists() {
            debug!("No skills directories found");
            // Still continue — workflow skills may exist even without regular skills
        }
    }

    let loader = SkillsLoader::new(user_skills_dir, builtin_skills_dir);
    let mut registry = match SkillsRegistry::from_loader(loader).await {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to load skills: {}", e);
            SkillsRegistry::new()
        }
    };

    // Discover skill-mode workflows and register alongside regular skills
    let workflows_dir = workspace.join("workflows");
    if workflows_dir.exists() {
        match crate::skills::discover_workflow_skills(&workflows_dir) {
            Ok(wf_skills) => {
                for skill in wf_skills {
                    registry.register(skill);
                }
            }
            Err(e) => {
                warn!("Failed to discover workflow skills: {}", e);
            }
        }
    }

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

/// Find the builtin skills directory.
///
/// Resolution order:
/// 1. `GASKET_SKILLS_DIR` environment variable
/// 2. Executable-relative heuristic (for dev builds)
/// 3. Current working directory fallback
pub fn find_builtin_skills_dir() -> Option<PathBuf> {
    // 1. Environment variable override (production deployments)
    if let Ok(env_dir) = std::env::var("GASKET_SKILLS_DIR") {
        let candidate = PathBuf::from(env_dir);
        if candidate.exists() {
            info!(
                "Found builtin skills from GASKET_SKILLS_DIR at {:?}",
                candidate
            );
            return Some(candidate);
        }
        warn!(
            "GASKET_SKILLS_DIR set to {:?} but directory does not exist",
            candidate
        );
    }

    // 2. Executable-relative heuristic (development builds)
    if let Ok(exe) = std::env::current_exe() {
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

    // 3. Current working directory fallback
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("engine").join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
        let candidate = cwd.join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
    }

    None
}
