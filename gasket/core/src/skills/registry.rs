use crate::search::top_k_similar;
use crate::search::TextEmbedder;
use crate::skills::{Skill, SkillsLoader};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use tracing::{debug, info};

/// Registry for managing loaded skills with semantic routing support.
///
/// The registry stores skill embeddings for fast Top-K retrieval based on
/// cosine similarity to the user query. Embeddings are computed once at
/// startup and cached in memory.
pub struct SkillsRegistry {
    /// All loaded skills (name -> skill)
    skills: HashMap<String, Skill>,

    /// Skills loader
    loader: Option<SkillsLoader>,

    /// Cached embeddings: (skill_name, embedding_vector)
    embeddings: OnceLock<Vec<(String, Vec<f32>)>>,
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
            embeddings: OnceLock::new(),
        }
    }

    /// Create a registry and load skills from directories
    pub async fn from_loader(loader: SkillsLoader) -> Result<Self> {
        let mut registry = Self::new();
        registry.loader = Some(loader);
        registry.load_skills().await?;
        Ok(registry)
    }

    /// Load skills using the configured loader
    pub async fn load_skills(&mut self) -> Result<()> {
        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Skills loader not configured"))?;

        let skills = loader.load_all().await?;
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
            info!("Overriding existing skill: {}", name);
        }

        debug!("Registering skill: {}", name);
        // Invalidate cached embeddings when skills change
        self.embeddings = OnceLock::new();
        self.skills.insert(name, skill);
    }

    /// Unregister a skill by name
    pub fn unregister(&mut self, name: &str) -> Option<Skill> {
        debug!("Unregistering skill: {}", name);
        // Invalidate cached embeddings when skills change
        self.embeddings = OnceLock::new();
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
        self.skills.values().filter(|s| !s.is_available()).collect()
    }

    /// Get skills that should always be loaded
    pub fn get_always_load_skills(&self) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.always_load() && s.is_available())
            .collect()
    }

    /// Initialize embeddings for all available skills.
    ///
    /// This should be called once after all skills are loaded.
    /// Uses the skill's description text to generate embeddings.
    pub fn initialize_embeddings(&self, embedder: &TextEmbedder) {
        if self.embeddings.get().is_some() {
            debug!("Skill embeddings already initialized, skipping");
            return;
        }

        let available_skills: Vec<_> = self.list_available();
        let mut embeddings = Vec::with_capacity(available_skills.len());

        for skill in available_skills {
            match embedder.embed(skill.description()) {
                Ok(vec) => {
                    embeddings.push((skill.name().to_string(), vec));
                    debug!("Generated embedding for skill: {}", skill.name());
                }
                Err(e) => {
                    tracing::warn!("Failed to embed skill '{}': {}", skill.name(), e);
                }
            }
        }

        if self.embeddings.set(embeddings).is_err() {
            debug!("Skill embeddings were already set by another thread");
        } else {
            info!("Initialized embeddings for {} skills", self.skills.len());
        }
    }

    /// Get the top-K most relevant skills for a query.
    ///
    /// Uses cosine similarity between the query embedding and cached skill embeddings.
    /// Returns skill names and their similarity scores.
    pub fn get_top_k(&self, query_vec: &[f32], k: usize) -> Vec<(&Skill, f32)> {
        let embeddings = match self.embeddings.get() {
            Some(e) => e,
            None => {
                // Fallback: return always-load skills if embeddings not initialized
                debug!("Skill embeddings not initialized, returning always-load skills");
                return self
                    .get_always_load_skills()
                    .into_iter()
                    .map(|s| (s, 1.0))
                    .collect();
            }
        };

        let top_names = top_k_similar(query_vec, embeddings, k);
        top_names
            .into_iter()
            .filter_map(|(name, score)| self.skills.get(name).map(|s| (s, score)))
            .collect()
    }

    /// Generate skill summary for agent context
    pub async fn generate_context_summary(&self) -> String {
        let mut summary = String::new();

        // Add always-load skills with full content
        let always_skills = self.get_always_load_skills();
        if !always_skills.is_empty() {
            summary.push_str("## Always-Loaded Skills\n\n");
            for skill in always_skills {
                summary.push_str(&format!("### {}\n\n", skill.name()));
                // Use load_content() which returns eagerly cached content for always-load skills
                summary.push_str(&skill.load_content().await);
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
            summary.push_str(
                "Available skills (use `read_file` to load full content when needed):\n\n",
            );

            for skill in on_demand_skills {
                summary.push_str(&skill.brief_summary());
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
        let metadata = SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            always,
            ..Default::default()
        };

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
        let metadata = SkillMetadata {
            name: "unavailable".to_string(),
            description: "Unavailable skill".to_string(),
            bins: vec!["nonexistent-binary-xyz".to_string()],
            ..Default::default()
        };
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

        let rt = tokio::runtime::Runtime::new().unwrap();
        let summary = rt.block_on(registry.generate_context_summary());

        assert!(summary.contains("Always-Loaded Skills"));
        assert!(summary.contains("On-Demand Skills"));
        assert!(summary.contains("github"));
        assert!(summary.contains("weather"));
    }
}
