use crate::skills::{Skill, SkillMetadata};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
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
pub async fn parse_skill_file(
    mut reader: BufReader<tokio::fs::File>,
    path: PathBuf,
) -> Result<Skill> {
    let (metadata, markdown_content) = parse_frontmatter(&mut reader).await?;

    // Lazy-load: skip content for on-demand skills to save memory
    if !metadata.always {
        Ok(Skill::new_lazy(metadata, path))
    } else {
        Ok(Skill::new(metadata, markdown_content, path))
    }
}

/// Parse YAML frontmatter and extract metadata + content
async fn parse_frontmatter(
    reader: &mut BufReader<tokio::fs::File>,
) -> Result<(SkillMetadata, String)> {
    let mut lines = reader.lines();

    // Read the first line
    let first_line = match lines.next_line().await? {
        Some(line) => line,
        None => return Ok((SkillMetadata::default(), String::new())),
    };

    // Check if file starts with ---
    if first_line.trim() != "---" {
        // No frontmatter, use default metadata
        let mut content = first_line;
        while let Some(line) = lines.next_line().await? {
            content.push('\n');
            content.push_str(&line);
        }
        return Ok((SkillMetadata::default(), content));
    }

    let mut yaml_content = String::new();
    let mut frontmatter_closed = false;

    // Read YAML content until closing ---
    while let Some(line) = lines.next_line().await? {
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
        while let Some(line) = lines.next_line().await? {
            content.push('\n');
            content.push_str(&line);
        }
        return Ok((SkillMetadata::default(), content));
    }

    // Parse YAML
    let metadata: SkillMetadata = serde_yaml::from_str(&yaml_content)
        .with_context(|| format!("Failed to parse YAML frontmatter: {}", yaml_content))?;

    // Extract Markdown content (everything after closing ---)
    let mut markdown_content = String::new();
    let mut first_non_empty = false;

    while let Some(line) = lines.next_line().await? {
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
    /// User skills directory (e.g., ~/.gasket/skills/)
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
    pub async fn load_all(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        // Load built-in skills
        debug!("Loading built-in skills from {:?}", self.builtin_skills_dir);
        if self.builtin_skills_dir.exists() {
            self.load_from_dir(&self.builtin_skills_dir, &mut skills)
                .await?;
        }

        // Load user skills
        debug!("Loading user skills from {:?}", self.user_skills_dir);
        if self.user_skills_dir.exists() {
            self.load_from_dir(&self.user_skills_dir, &mut skills)
                .await?;
        }

        debug!("Loaded {} skills total", skills.len());
        Ok(skills)
    }

    /// Load skills from a specific directory
    ///
    /// Supports two layouts:
    /// 1. Flat: `skills/weather.md`
    /// 2. Nested: `skills/weather/SKILL.md`
    async fn load_from_dir(&self, dir: &Path, skills: &mut Vec<Skill>) -> Result<()> {
        let mut entries = fs::read_dir(dir)
            .await
            .with_context(|| format!("Failed to read skills directory: {:?}", dir))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Check if it's a direct .md file (flat layout)
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                match self.load_skill(&path).await {
                    Ok(skill) => {
                        debug!("Loaded skill: {} from {:?}", skill.name(), path);
                        skills.push(skill);
                    }
                    Err(e) => {
                        warn!("Failed to load skill from {:?}: {}", path, e);
                    }
                }
            }
            // Check if it's a directory with SKILL.md inside (nested layout)
            else if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    match self.load_skill(&skill_file).await {
                        Ok(skill) => {
                            debug!("Loaded skill: {} from {:?}", skill.name(), skill_file);
                            skills.push(skill);
                        }
                        Err(e) => {
                            warn!("Failed to load skill from {:?}: {}", skill_file, e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn load_skill(&self, path: &Path) -> Result<Skill> {
        let file = fs::File::open(path)
            .await
            .with_context(|| format!("Failed to open skill file: {:?}", path))?;
        let reader = BufReader::new(file);

        parse_skill_file(reader, path.to_path_buf()).await
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
    use std::io::Write;

    use super::*;

    #[tokio::test]
    async fn test_parse_frontmatter_full() {
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
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.as_file().write_all(content.as_bytes()).unwrap();

        let file = fs::File::open(temp_file.path()).await.unwrap();
        let mut reader = BufReader::new(file);
        let (metadata, markdown) = parse_frontmatter(&mut reader).await.unwrap();

        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill");
        assert!(metadata.always);
        assert_eq!(metadata.bins, vec!["git", "gh"]);
        assert_eq!(metadata.env_vars, vec!["GITHUB_TOKEN"]);
        assert!(markdown.contains("# Test Skill"));
    }

    #[tokio::test]
    async fn test_parse_frontmatter_minimal() {
        let content = r#"---
name: minimal
description: Minimal skill
---

# Content
"#;
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.as_file().write_all(content.as_bytes()).unwrap();

        let file = fs::File::open(temp_file.path()).await.unwrap();
        let mut reader = BufReader::new(file);
        let (metadata, markdown) = parse_frontmatter(&mut reader).await.unwrap();

        assert_eq!(metadata.name, "minimal");
        assert_eq!(metadata.description, "Minimal skill");
        assert!(!metadata.always);
        assert!(metadata.bins.is_empty());
        assert!(markdown.contains("# Content"));
    }

    #[tokio::test]
    async fn test_parse_no_frontmatter() {
        let content = "# No Frontmatter\n\nJust content.";
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.as_file().write_all(content.as_bytes()).unwrap();

        let file = fs::File::open(temp_file.path()).await.unwrap();
        let mut reader = BufReader::new(file);
        let (metadata, markdown) = parse_frontmatter(&mut reader).await.unwrap();

        assert_eq!(metadata.name, "");
        assert!(markdown.contains("# No Frontmatter"));
    }

    #[tokio::test]
    async fn test_parse_skill_file() {
        let content = r#"---
name: github
description: GitHub operations
---

# GitHub Skill

Use `gh` CLI for GitHub operations.
"#;
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.as_file().write_all(content.as_bytes()).unwrap();

        let file = fs::File::open(temp_file.path()).await.unwrap();
        let reader = BufReader::new(file);
        let skill = parse_skill_file(reader, PathBuf::from(temp_file.path()))
            .await
            .unwrap();

        assert_eq!(skill.name(), "github");
        assert_eq!(skill.description(), "GitHub operations");
        // always: false (default) → lazy loaded, content is empty
        assert!(skill.content().is_empty());
    }

    #[tokio::test]
    async fn test_parse_skill_file_always_load() {
        let content = r#"---
name: core-skill
description: Always-loaded skill
always: true
---

# Core Skill

This content is eagerly loaded.
"#;
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.as_file().write_all(content.as_bytes()).unwrap();

        let file = fs::File::open(temp_file.path()).await.unwrap();
        let reader = BufReader::new(file);
        let skill = parse_skill_file(reader, PathBuf::from(temp_file.path()))
            .await
            .unwrap();

        assert_eq!(skill.name(), "core-skill");
        assert!(skill.always_load());
        assert!(!skill.content().is_empty());
        assert!(skill.content().contains("Core Skill"));
    }
}
