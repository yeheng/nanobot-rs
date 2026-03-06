use crate::skills::SkillMetadata;
use std::path::PathBuf;
use tokio::fs;
use tracing::warn;

/// Represents a loaded skill.
///
/// Skills with `always: true` eagerly load their full content at startup.
/// Skills with `always: false` only store metadata and path; the content
/// field is empty and should be read from disk on demand via `read_file`.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill metadata
    metadata: SkillMetadata,

    /// Full skill content (Markdown). Empty for lazy-loaded (on-demand) skills.
    content: String,

    /// File path to the skill (for read_file access)
    path: PathBuf,

    /// Whether the skill is available (dependencies satisfied)
    available: bool,

    /// Missing dependencies (if any)
    missing_deps: Vec<String>,
}

impl Skill {
    /// Create a new skill with eagerly loaded content.
    pub fn new(metadata: SkillMetadata, content: String, path: PathBuf) -> Self {
        let missing_deps = metadata.missing_dependencies();
        let available = missing_deps.is_empty();

        Self {
            metadata,
            content,
            path,
            available,
            missing_deps,
        }
    }

    /// Create a lazy skill — only metadata and path, content is empty.
    ///
    /// Used for on-demand skills (`always: false`) to save memory at startup.
    pub fn new_lazy(metadata: SkillMetadata, path: PathBuf) -> Self {
        let missing_deps = metadata.missing_dependencies();
        let available = missing_deps.is_empty();

        Self {
            metadata,
            content: String::new(),
            path,
            available,
            missing_deps,
        }
    }

    /// Get skill metadata
    pub fn metadata(&self) -> &SkillMetadata {
        &self.metadata
    }

    /// Get skill name
    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    /// Get skill description
    pub fn description(&self) -> &str {
        &self.metadata.description
    }

    /// Get full skill content (may be empty for lazy-loaded skills).
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Load and return content from disk. Falls back to cached content.
    pub async fn load_content(&self) -> String {
        if !self.content.is_empty() {
            return self.content.clone();
        }
        match fs::read_to_string(&self.path).await {
            Ok(raw) => {
                // Strip frontmatter if present
                if let Some(after_start) = raw.strip_prefix("---") {
                    if let Some(end) = after_start.find("---") {
                        let after = &after_start[end + 3..];
                        return after.trim_start().to_string();
                    }
                }
                raw
            }
            Err(e) => {
                warn!("Failed to read skill content from {:?}: {}", self.path, e);
                String::new()
            }
        }
    }

    /// Get skill file path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Check if skill is available (dependencies satisfied)
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Get missing dependencies
    pub fn missing_dependencies(&self) -> &[String] {
        &self.missing_deps
    }

    /// Check if skill should always be loaded
    pub fn always_load(&self) -> bool {
        self.metadata.always
    }

    /// Get skill summary (for context building)
    pub fn summary(&self) -> String {
        if self.available {
            format!(
                "- **{}**: {} (path: {})",
                self.metadata.name,
                self.metadata.description,
                self.path.display()
            )
        } else {
            format!(
                "- **{}**: {} (unavailable: {})",
                self.metadata.name,
                self.metadata.description,
                self.missing_deps.join(", ")
            )
        }
    }

    /// Get skill summary without path (for on-demand loading)
    pub fn brief_summary(&self) -> String {
        if self.available {
            format!(
                "- **{}**: {}",
                self.metadata.name, self.metadata.description
            )
        } else {
            format!(
                "- **{}**: {} (unavailable)",
                self.metadata.name, self.metadata.description
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_creation() {
        let metadata = SkillMetadata {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            ..Default::default()
        };

        let content = "# Test Skill\n\nThis is a test.".to_string();
        let path = PathBuf::from("/test/skill.md");

        let skill = Skill::new(metadata, content.clone(), path.clone());

        assert_eq!(skill.name(), "test-skill");
        assert_eq!(skill.description(), "A test skill");
        assert_eq!(skill.content(), &content);
        assert_eq!(skill.path(), &path);
        assert!(skill.is_available());
        assert!(!skill.always_load());
    }

    #[test]
    fn test_lazy_skill() {
        let metadata = SkillMetadata {
            name: "lazy-skill".to_string(),
            description: "A lazy skill".to_string(),
            ..Default::default()
        };

        let path = PathBuf::from("/test/lazy.md");
        let skill = Skill::new_lazy(metadata, path);

        assert_eq!(skill.name(), "lazy-skill");
        assert!(skill.content().is_empty());
        assert!(skill.is_available());
    }

    #[test]
    fn test_skill_summary() {
        let metadata = SkillMetadata {
            name: "github".to_string(),
            description: "GitHub operations".to_string(),
            bins: vec!["gh".to_string()],
            ..Default::default()
        };

        let skill = Skill::new(
            metadata,
            "content".to_string(),
            PathBuf::from("/skills/github.md"),
        );

        let summary = skill.summary();
        assert!(summary.contains("github"));
        assert!(summary.contains("GitHub operations"));
    }

    #[test]
    fn test_unavailable_skill() {
        let metadata = SkillMetadata {
            name: "unavailable".to_string(),
            bins: vec!["nonexistent-binary-xyz".to_string()],
            ..Default::default()
        };

        let skill = Skill::new(
            metadata,
            "content".to_string(),
            PathBuf::from("/skills/unavailable.md"),
        );

        assert!(!skill.is_available());
        assert!(!skill.missing_dependencies().is_empty());
    }
}
