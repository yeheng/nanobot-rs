use crate::skills::SkillMetadata;
use std::path::PathBuf;

/// Represents a loaded skill
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill metadata
    metadata: SkillMetadata,

    /// Full skill content (Markdown)
    content: String,

    /// File path to the skill (for read_file access)
    path: PathBuf,

    /// Whether the skill is available (dependencies satisfied)
    available: bool,

    /// Missing dependencies (if any)
    missing_deps: Vec<String>,
}

impl Skill {
    /// Create a new skill
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

    /// Get full skill content
    pub fn content(&self) -> &str {
        &self.content
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
        let mut metadata = SkillMetadata::default();
        metadata.name = "test-skill".to_string();
        metadata.description = "A test skill".to_string();

        let content = "# Test Skill\n\nThis is a test.".to_string();
        let path = PathBuf::from("/test/skill.md");

        let skill = Skill::new(metadata, content.clone(), path.clone());

        assert_eq!(skill.name(), "test-skill");
        assert_eq!(skill.description(), "A test skill");
        assert_eq!(skill.content(), &content);
        assert_eq!(skill.path(), &path);
        assert!(skill.is_available());
        assert!(skill.always_load() == false);
    }

    #[test]
    fn test_skill_summary() {
        let mut metadata = SkillMetadata::default();
        metadata.name = "github".to_string();
        metadata.description = "GitHub operations".to_string();
        metadata.bins = vec!["gh".to_string()];

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
        let mut metadata = SkillMetadata::default();
        metadata.name = "unavailable".to_string();
        metadata.bins = vec!["nonexistent-binary-xyz".to_string()];

        let skill = Skill::new(
            metadata,
            "content".to_string(),
            PathBuf::from("/skills/unavailable.md"),
        );

        assert!(!skill.is_available());
        assert!(!skill.missing_dependencies().is_empty());
    }
}
