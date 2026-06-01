//! Workflow-to-skill converter.
//!
//! Scans `workspace/workflows/` for YAML manifests with `mode: "skill"`
//! and converts them into `Skill` objects that are injected into the
//! agent's system prompt.  In skill mode the LLM executes workflow steps
//! autonomously through the normal kernel loop instead of using the
//! hard-coded state-machine / subagent spawning path.

use std::path::Path;

use tracing::{info, warn};

use crate::skills::{Skill, SkillMetadata};
use crate::tools::workflow::{load_workflow, Workflow, WorkflowMode};

/// Discover skill-mode workflows in `workflows_dir` and convert them to `Skill`s.
///
/// Only workflows whose `mode` field equals `"skill"` are picked up.
/// Tool-mode workflows (the default) are ignored here and handled by the
/// tool registry builder instead.
pub fn discover_workflow_skills(workflows_dir: &Path) -> anyhow::Result<Vec<Skill>> {
    let mut skills = Vec::new();

    if !workflows_dir.exists() {
        tracing::info!(
            "Workflows directory does not exist: {:?}, skipping workflow-skill discovery",
            workflows_dir
        );
        return Ok(skills);
    }

    let entries = std::fs::read_dir(workflows_dir).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read workflows directory {:?}: {}",
            workflows_dir,
            e
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!(
                "Failed to read directory entry in {:?}: {}",
                workflows_dir,
                e
            )
        })?;

        let path = entry.path();
        if path.is_dir() {
            continue;
        }

        let ext = path.extension().and_then(|s| s.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        match load_workflow(&path) {
            Ok(manifest) => {
                if manifest.mode != WorkflowMode::Skill {
                    continue;
                }

                let workflow = match Workflow::from_manifest(&manifest) {
                    Ok(wf) => wf,
                    Err(e) => {
                        warn!("Failed to validate workflow-skill from {:?}: {}", path, e);
                        continue;
                    }
                };

                let metadata = SkillMetadata {
                    name: workflow.name.clone(),
                    description: workflow.description.clone(),
                    always: workflow.always,
                    bins: Vec::new(),
                    env_vars: Vec::new(),
                    extra: Default::default(),
                };

                let content = workflow.to_skill_content();
                info!(
                    "Discovered workflow-skill '{}' from {:?}",
                    workflow.name, path
                );
                skills.push(Skill::new(metadata, content, path));
            }
            Err(e) => {
                warn!("Failed to load workflow from {:?}: {}", path, e);
            }
        }
    }

    Ok(skills)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn discover_filters_non_skill_mode() {
        let dir = tempfile::tempdir().unwrap();

        // Tool-mode workflow — should be ignored
        let tool_wf = r#"
name: "tool_mode_workflow"
description: "A tool mode workflow"
parameters:
  type: object
  properties: {}
start_step: "a"
steps:
  a:
    prompt: "hello"
    next: "DONE"
"#;
        let mut f1 = std::fs::File::create(dir.path().join("tool.yaml")).unwrap();
        f1.write_all(tool_wf.as_bytes()).unwrap();

        // Skill-mode workflow — should be picked up
        let skill_wf = r#"
name: "skill_mode_workflow"
description: "A skill mode workflow"
mode: "skill"
parameters:
  type: object
  properties: {}
start_step: "a"
steps:
  a:
    prompt: "hello"
    next: "DONE"
"#;
        let mut f2 = std::fs::File::create(dir.path().join("skill.yaml")).unwrap();
        f2.write_all(skill_wf.as_bytes()).unwrap();

        let skills = discover_workflow_skills(dir.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "skill_mode_workflow");
        assert!(skills[0].always_load());
        assert!(skills[0]
            .content()
            .contains("Workflow: skill_mode_workflow"));
    }

    #[test]
    fn discover_empty_dir_ok() {
        let dir = tempfile::tempdir().unwrap();
        let skills = discover_workflow_skills(dir.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn discover_missing_dir_ok() {
        let skills = discover_workflow_skills(Path::new("/nonexistent/path")).unwrap();
        assert!(skills.is_empty());
    }
}
