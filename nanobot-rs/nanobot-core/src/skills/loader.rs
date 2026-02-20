use crate::skills::{Skill, SkillMetadata};
use anyhow::{Context, Result};
use std::fs;
use std::io::BufRead;
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
pub fn parse_skill_file(mut reader: impl BufRead, path: PathBuf) -> Result<Skill> {
    let (metadata, markdown_content) = parse_frontmatter(&mut reader)?;

    Ok(Skill::new(metadata, markdown_content, path))
}

/// Parse YAML frontmatter and extract metadata + content
fn parse_frontmatter(reader: &mut impl BufRead) -> Result<(SkillMetadata, String)> {
    let mut lines = reader.lines();

    // Read the first line
    let first_line = match lines.next() {
        Some(Ok(line)) => line,
        _ => return Ok((SkillMetadata::default(), String::new())),
    };

    // Check if file starts with ---
    if first_line.trim() != "---" {
        // No frontmatter, use default metadata
        let mut content = first_line;
        for line in lines {
            content.push('\n');
            content.push_str(&line?);
        }
        return Ok((SkillMetadata::default(), content));
    }

    let mut yaml_content = String::new();
    let mut frontmatter_closed = false;

    // Read YAML content until closing ---
    for line in &mut lines {
        let line = line?;
        if line.trim() == "---" {
            frontmatter_closed = true;
            break;
        }
        yaml_content.push_str(&line);
        yaml_content.push('\n');
    }

    if !frontmatter_closed {
        warn!("Unclosed frontmatter in skill file");
        let mut content = format!("---\n{}", yaml_content);
        for line in lines {
            content.push('\n');
            content.push_str(&line?);
        }
        return Ok((SkillMetadata::default(), content));
    }

    // Parse YAML
    let metadata: SkillMetadata = serde_yaml::from_str(&yaml_content)
        .with_context(|| format!("Failed to parse YAML frontmatter: {}", yaml_content))?;

    // Extract Markdown content (everything after closing ---)
    let mut markdown_content = String::new();
    let mut first_non_empty = false;

    for line in lines {
        let line = line?;
        if !first_non_empty {
            if line.trim().is_empty() {
                continue;
            }
            first_non_empty = true;
        }
        if !markdown_content.is_empty() {
            markdown_content.push('\n');
        }
        markdown_content.push_str(&line);
    }

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

    fn load_skill(&self, path: &Path) -> Result<Skill> {
        let file = fs::File::open(path)
            .with_context(|| format!("Failed to open skill file: {:?}", path))?;
        let reader = std::io::BufReader::new(file);

        parse_skill_file(reader, path.to_path_buf())
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
    use std::io::Cursor;

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
        let mut reader = Cursor::new(content);
        let (metadata, markdown) = parse_frontmatter(&mut reader).unwrap();

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
        let mut reader = Cursor::new(content);
        let (metadata, markdown) = parse_frontmatter(&mut reader).unwrap();

        assert_eq!(metadata.name, "minimal");
        assert_eq!(metadata.description, "Minimal skill");
        assert_eq!(metadata.always, false);
        assert!(metadata.bins.is_empty());
        assert!(markdown.contains("# Content"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "# No Frontmatter\n\nJust content.";
        let mut reader = Cursor::new(content);
        let (metadata, markdown) = parse_frontmatter(&mut reader).unwrap();

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
        let reader = Cursor::new(content);
        let skill = parse_skill_file(reader, PathBuf::from("/test/github.md")).unwrap();

        assert_eq!(skill.name(), "github");
        assert_eq!(skill.description(), "GitHub operations");
        assert!(skill.content().contains("GitHub Skill"));
    }
}
