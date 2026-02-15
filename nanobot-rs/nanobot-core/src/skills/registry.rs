use crate::skills::{Skill, SkillsLoader};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

/// Registry for managing loaded skills
pub struct SkillsRegistry {
    /// All loaded skills (name -> skill)
    skills: HashMap<String, Skill>,

    /// Skills loader
    loader: Option<SkillsLoader>,
}

impl Default for SkillsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillsRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            loader: None,
        }
    }

    /// Create a registry and load skills from directories
    pub fn from_loader(loader: SkillsLoader) -> Result<Self> {
        let mut registry = Self::new();
        registry.loader = Some(loader);
        registry.load_skills()?;
        Ok(registry)
    }

    /// Load skills using the configured loader
    pub fn load_skills(&mut self) -> Result<()> {
        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Skills loader not configured"))?;

        let skills = loader.load_all()?;
        info!("Loaded {} skills", skills.len());

        for skill in skills {
            self.register(skill);
        }

        Ok(())
    }

    /// Register a skill
    pub fn register(&mut self, skill: Skill) {
        let name = skill.name().to_string();

        if self.skills.contains_key(&name) {
            warn!("Overriding existing skill: {}", name);
        }

        debug!("Registering skill: {}", name);
        self.skills.insert(name, skill);
    }

    /// Unregister a skill by name
    pub fn unregister(&mut self, name: &str) -> Option<Skill> {
        debug!("Unregistering skill: {}", name);
        self.skills.remove(name)
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Check if a skill exists
    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }

    /// List all skills
    pub fn list_all(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// List only available skills (dependencies satisfied)
    pub fn list_available(&self) -> Vec<&Skill> {
        self.skills.values().filter(|s| s.is_available()).collect()
    }

    /// List unavailable skills
    pub fn list_unavailable(&self) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| !s.is_available())
            .collect()
    }

    /// Get skills that should always be loaded
    pub fn get_always_load_skills(&self) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.always_load() && s.is_available())
            .collect()
    }

    /// Generate skill summary for agent context
    pub fn generate_context_summary(&self) -> String {
        let mut summary = String::new();

        // Add always-load skills with full content
        let always_skills = self.get_always_load_skills();
        if !always_skills.is_empty() {
            summary.push_str("## Always-Loaded Skills\n\n");
            for skill in always_skills {
                summary.push_str(&format!("### {}\n\n", skill.name()));
                summary.push_str(skill.content());
                summary.push_str("\n\n");
            }
        }

        // Add other available skills as summaries
        let on_demand_skills: Vec<_> = self
            .list_available()
            .into_iter()
            .filter(|s| !s.always_load())
            .collect();

        if !on_demand_skills.is_empty() {
            summary.push_str("## On-Demand Skills\n\n");
            summary.push_str("The following skills are available. Use `read_file` to load their full content:\n\n");

            for skill in on_demand_skills {
                summary.push_str(&skill.summary());
                summary.push('\n');
            }
        }

        // Add unavailable skills with warnings
        let unavailable = self.list_unavailable();
        if !unavailable.is_empty() {
            summary.push_str("\n## Unavailable Skills\n\n");
            summary.push_str("The following skills have missing dependencies:\n\n");

            for skill in unavailable {
                summary.push_str(&format!(
                    "- **{}**: missing {}",
                    skill.name(),
                    skill.missing_dependencies().join(", ")
                ));
                summary.push('\n');
            }
        }

        summary
    }

    /// Get total number of skills
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Get user skills directory (if loader is configured)
    pub fn user_skills_dir(&self) -> Option<&Path> {
        self.loader.as_ref().map(|l| l.user_skills_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillMetadata;
    use std::path::PathBuf;

    fn create_test_skill(name: &str, description: &str, always: bool) -> Skill {
        let mut metadata = SkillMetadata::default();
        metadata.name = name.to_string();
        metadata.description = description.to_string();
        metadata.always = always;

        Skill::new(
            metadata,
            format!("# {}\n\n{}", name, description),
            PathBuf::from(format!("/skills/{}.md", name)),
        )
    }

    #[test]
    fn test_registry_creation() {
        let registry = SkillsRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = SkillsRegistry::new();
        let skill = create_test_skill("test", "Test skill", false);

        registry.register(skill);
        assert_eq!(registry.len(), 1);

        let retrieved = registry.get("test");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), "test");
    }

    #[test]
    fn test_unregister() {
        let mut registry = SkillsRegistry::new();
        let skill = create_test_skill("test", "Test skill", false);

        registry.register(skill);
        assert_eq!(registry.len(), 1);

        let removed = registry.unregister("test");
        assert!(removed.is_some());
        assert!(registry.is_empty());
    }

    #[test]
    fn test_list_available() {
        let mut registry = SkillsRegistry::new();

        // Available skill
        let available = create_test_skill("available", "Available skill", false);
        registry.register(available);

        // Unavailable skill (has missing dependency)
        let mut metadata = SkillMetadata::default();
        metadata.name = "unavailable".to_string();
        metadata.description = "Unavailable skill".to_string();
        metadata.bins = vec!["nonexistent-binary-xyz".to_string()];
        let unavailable = Skill::new(
            metadata,
            "content".to_string(),
            PathBuf::from("/skills/unavailable.md"),
        );
        registry.register(unavailable);

        let available_list = registry.list_available();
        assert_eq!(available_list.len(), 1);
        assert_eq!(available_list[0].name(), "available");
    }

    #[test]
    fn test_always_load_skills() {
        let mut registry = SkillsRegistry::new();

        let always = create_test_skill("always", "Always skill", true);
        let on_demand = create_test_skill("on-demand", "On-demand skill", false);

        registry.register(always);
        registry.register(on_demand);

        let always_skills = registry.get_always_load_skills();
        assert_eq!(always_skills.len(), 1);
        assert_eq!(always_skills[0].name(), "always");
    }

    #[test]
    fn test_generate_context_summary() {
        let mut registry = SkillsRegistry::new();

        let always = create_test_skill("github", "GitHub operations", true);
        let on_demand = create_test_skill("weather", "Weather queries", false);

        registry.register(always);
        registry.register(on_demand);

        let summary = registry.generate_context_summary();

        assert!(summary.contains("Always-Loaded Skills"));
        assert!(summary.contains("On-Demand Skills"));
        assert!(summary.contains("github"));
        assert!(summary.contains("weather"));
    }
}
