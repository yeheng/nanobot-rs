use crate::skills::{Skill, SkillMetadata};
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, Cursor};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Parses YAML frontmatter from a Markdown file
///
/// Format:
/// ```markdown
/// ---
/// name: my-skill
/// description: My skill description
/// always: false
/// bins: ["git"]
/// env_vars: ["GITHUB_TOKEN"]
/// ---
///
/// # Skill Content
/// ...
/// ```
pub fn parse_skill_file(content: &str, path: PathBuf) -> Result<Skill> {
    let (metadata, markdown_content) = parse_frontmatter(content)?;

    Ok(Skill::new(metadata, markdown_content, path))
}

/// Parse YAML frontmatter and extract metadata + content
fn parse_frontmatter(content: &str) -> Result<(SkillMetadata, String)> {
    let reader = Cursor::new(content);
    let lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;

    // Check if file starts with ---
    if lines.is_empty() || lines[0].trim() != "---" {
        // No frontmatter, use default metadata
        return Ok((SkillMetadata::default(), content.to_string()));
    }

    // Find closing ---
    let end_index = lines[1..]
        .iter()
        .position(|line| line.trim() == "---")
        .map(|i| i + 1);

    let end_index = match end_index {
        Some(i) => i,
        None => {
            warn!("Unclosed frontmatter in skill file");
            return Ok((SkillMetadata::default(), content.to_string()));
        }
    };

    // Extract YAML content
    let yaml_content: String = lines[1..end_index].join("\n");

    // Parse YAML
    let metadata: SkillMetadata = serde_yaml::from_str(&yaml_content)
        .with_context(|| format!("Failed to parse YAML frontmatter: {}", yaml_content))?;

    // Extract Markdown content (everything after closing ---)
    let markdown_start = end_index + 1;
    let markdown_content: String = if markdown_start < lines.len() {
        // Skip empty lines at the beginning
        lines[markdown_start..]
            .iter()
            .skip_while(|line| line.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };

    Ok((metadata, markdown_content))
}

/// Skills loader that loads skills from directories
pub struct SkillsLoader {
    /// User skills directory (e.g., ~/.nanobot/skills/)
    user_skills_dir: PathBuf,

    /// Built-in skills directory
    builtin_skills_dir: PathBuf,
}

impl SkillsLoader {
    /// Create a new skills loader
    pub fn new(user_skills_dir: PathBuf, builtin_skills_dir: PathBuf) -> Self {
        Self {
            user_skills_dir,
            builtin_skills_dir,
        }
    }

    /// Load all skills from both user and builtin directories
    pub fn load_all(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        // Load built-in skills
        debug!("Loading built-in skills from {:?}", self.builtin_skills_dir);
        if self.builtin_skills_dir.exists() {
            self.load_from_dir(&self.builtin_skills_dir, &mut skills)?;
        }

        // Load user skills
        debug!("Loading user skills from {:?}", self.user_skills_dir);
        if self.user_skills_dir.exists() {
            self.load_from_dir(&self.user_skills_dir, &mut skills)?;
        }

        debug!("Loaded {} skills total", skills.len());
        Ok(skills)
    }

    /// Load skills from a specific directory
    fn load_from_dir(&self, dir: &Path, skills: &mut Vec<Skill>) -> Result<()> {
        let entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read skills directory: {:?}", dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only process .md files
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                match self.load_skill(&path) {
                    Ok(skill) => {
                        debug!("Loaded skill: {} from {:?}", skill.name(), path);
                        skills.push(skill);
                    }
                    Err(e) => {
                        warn!("Failed to load skill from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Load a single skill from a file
    fn load_skill(&self, path: &Path) -> Result<Skill> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read skill file: {:?}", path))?;

        parse_skill_file(&content, path.to_path_buf())
    }

    /// Get user skills directory
    pub fn user_skills_dir(&self) -> &Path {
        &self.user_skills_dir
    }

    /// Get builtin skills directory
    pub fn builtin_skills_dir(&self) -> &Path {
        &self.builtin_skills_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_full() {
        let content = r#"---
name: test-skill
description: A test skill
always: true
bins: ["git", "gh"]
env_vars: ["GITHUB_TOKEN"]
---

# Test Skill

This is the skill content.
"#;

        let (metadata, markdown) = parse_frontmatter(content).unwrap();

        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill");
        assert_eq!(metadata.always, true);
        assert_eq!(metadata.bins, vec!["git", "gh"]);
        assert_eq!(metadata.env_vars, vec!["GITHUB_TOKEN"]);
        assert!(markdown.contains("# Test Skill"));
    }

    #[test]
    fn test_parse_frontmatter_minimal() {
        let content = r#"---
name: minimal
description: Minimal skill
---

# Content
"#;

        let (metadata, markdown) = parse_frontmatter(content).unwrap();

        assert_eq!(metadata.name, "minimal");
        assert_eq!(metadata.description, "Minimal skill");
        assert_eq!(metadata.always, false);
        assert!(metadata.bins.is_empty());
        assert!(markdown.contains("# Content"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "# No Frontmatter\n\nJust content.";

        let (metadata, markdown) = parse_frontmatter(content).unwrap();

        assert_eq!(metadata.name, "");
        assert_eq!(markdown, content);
    }

    #[test]
    fn test_parse_skill_file() {
        let content = r#"---
name: github
description: GitHub operations
---

# GitHub Skill

Use `gh` CLI for GitHub operations.
"#;

        let skill = parse_skill_file(content, PathBuf::from("/test/github.md")).unwrap();

        assert_eq!(skill.name(), "github");
        assert_eq!(skill.description(), "GitHub operations");
        assert!(skill.content().contains("GitHub Skill"));
    }
}
