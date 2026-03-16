//! Model Registry for managing model profiles
//!
//! Provides lookup and management for named model profiles that can be
//! used for dynamic model switching via the `switch_model` tool.

use std::collections::HashMap;

use super::agent::{AgentsConfig, ModelProfile};

/// Registry for managing model profiles
///
/// Stores model profiles by ID and provides lookup methods.
/// The default model ID is extracted from the agent config's model field
/// (format: "provider/model" -> extracts model ID or uses as-is).
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Model profiles indexed by ID
    profiles: HashMap<String, ModelProfile>,

    /// Default model ID (extracted from agents.defaults.model)
    default_model_id: Option<String>,
}

impl ModelRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
            default_model_id: None,
        }
    }

    /// Create a registry from agent configuration
    pub fn from_config(config: &AgentsConfig) -> Self {
        let mut registry = Self::new();

        // Add all model profiles
        for (id, profile) in &config.models {
            registry.profiles.insert(id.clone(), profile.clone());
        }

        // Extract default model ID from agents.defaults.model
        // Format can be "provider/model" or just a model profile ID
        if let Some(ref model) = config.defaults.model {
            // If the model string matches a profile ID, use it
            // Otherwise, check if it's a provider/model format
            if registry.profiles.contains_key(model) {
                registry.default_model_id = Some(model.clone());
            } else {
                // Try to find a profile that matches the provider/model pattern
                // For now, we'll store the full string as the default
                registry.default_model_id = Some(model.clone());
            }
        }

        registry
    }

    /// Get a model profile by ID
    pub fn get_profile(&self, id: &str) -> Option<&ModelProfile> {
        self.profiles.get(id)
    }

    /// Get the default model profile
    ///
    /// Returns the profile for the default model ID if it exists in the profiles map.
    /// If not found, returns None.
    pub fn get_default_profile(&self) -> Option<(&str, &ModelProfile)> {
        self.default_model_id
            .as_ref()
            .and_then(|id| self.profiles.get(id).map(|p| (id.as_str(), p)))
    }

    /// Get the default model ID
    pub fn get_default_model_id(&self) -> Option<&str> {
        self.default_model_id.as_deref()
    }

    /// List all available model IDs
    pub fn list_available_models(&self) -> Vec<&str> {
        self.profiles.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a model profile exists
    pub fn contains(&self, id: &str) -> bool {
        self.profiles.contains_key(id)
    }

    /// Get the number of registered profiles
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Add a model profile
    pub fn insert(&mut self, id: String, profile: ModelProfile) {
        self.profiles.insert(id, profile);
    }

    /// Set the default model ID
    pub fn set_default_model_id(&mut self, id: Option<String>) {
        self.default_model_id = id;
    }

    /// Get a model profile with fallback to default.
    ///
    /// Lookup order:
    /// 1. If `model_id` is provided and found in profiles, return it
    /// 2. If not found or not provided, fallback to default profile
    /// 3. Returns None if neither is available
    ///
    /// # Arguments
    /// * `model_id` - Optional model ID to look up
    ///
    /// # Returns
    /// Tuple of (profile_id, profile) if found, None otherwise
    pub fn get_profile_with_fallback<'a>(
        &'a self,
        model_id: Option<&'a str>,
    ) -> Option<(&'a str, &'a ModelProfile)> {
        match model_id {
            Some(id) => {
                // First try exact match
                if let Some(profile) = self.profiles.get(id) {
                    return Some((id, profile));
                }
                // Fallback to default
                self.get_default_profile()
            }
            None => self.get_default_profile(),
        }
    }
}

/// Smart model selection based on task content analysis.
///
/// This module provides capability-based model selection when the
/// `smart-model-selection` feature is enabled.
#[cfg(feature = "smart-model-selection")]
impl ModelRegistry {
    /// Capability keyword mappings.
    ///
    /// Maps task keywords to capability tags for model selection.
    const CAPABILITY_KEYWORDS: &[(&str, &str)] = &[
        // Code-related
        ("code", "code"),
        ("programming", "code"),
        ("debug", "code"),
        ("debugging", "code"),
        ("implement", "code"),
        ("refactor", "code"),
        ("function", "code"),
        ("class", "code"),
        ("module", "code"),
        ("api", "code"),
        // Reasoning-related
        ("reasoning", "reasoning"),
        ("analyze", "reasoning"),
        ("analysis", "reasoning"),
        ("think", "reasoning"),
        ("explain", "reasoning"),
        ("compare", "reasoning"),
        ("evaluate", "reasoning"),
        ("complex", "reasoning"),
        // Creative-related
        ("creative", "creative"),
        ("write", "creative"),
        ("writing", "creative"),
        ("story", "creative"),
        ("article", "creative"),
        ("blog", "creative"),
        ("draft", "creative"),
        // Fast/simple tasks
        ("fast", "fast"),
        ("quick", "fast"),
        ("simple", "fast"),
        ("basic", "fast"),
        ("short", "fast"),
        // Math/calculation
        ("math", "math"),
        ("calculate", "math"),
        ("calculation", "math"),
        ("equation", "math"),
        // Data processing
        ("data", "data"),
        ("process", "data"),
        ("transform", "data"),
        ("parse", "data"),
        ("extract", "data"),
    ];

    /// Select a model based on task content analysis.
    ///
    /// Analyzes the task description to determine required capabilities
    /// and selects the best matching model profile.
    ///
    /// # Arguments
    /// * `task` - The task description to analyze
    ///
    /// # Returns
    /// The best matching model profile, or the default if no match found
    pub fn select_by_capability(&self, task: &str) -> Option<(&str, &ModelProfile)> {
        // Analyze task content to determine required capabilities
        let required_caps = self.analyze_task_capabilities(task);

        if required_caps.is_empty() {
            // No specific capability detected, use default
            return self.get_default_profile();
        }

        // Find best matching model
        self.find_best_match(&required_caps)
    }

    /// Analyze task content to extract required capabilities.
    fn analyze_task_capabilities(&self, task: &str) -> Vec<String> {
        let task_lower = task.to_lowercase();
        let mut capabilities: Vec<String> = Vec::new();

        for (keyword, capability) in Self::CAPABILITY_KEYWORDS {
            if task_lower.contains(keyword) {
                capabilities.push(capability.to_string());
            }
        }

        // Deduplicate while preserving order
        capabilities.sort();
        capabilities.dedup();

        capabilities
    }

    /// Find the best matching model for required capabilities.
    ///
    /// Scoring: Each matching capability adds 1 point.
    /// Returns the model with the highest score.
    fn find_best_match(&self, required_caps: &[String]) -> Option<(&str, &ModelProfile)> {
        let mut best_match: Option<(&str, &ModelProfile, usize)> = None;

        for (id, profile) in &self.profiles {
            // Calculate match score
            let score = profile
                .capabilities
                .iter()
                .filter(|cap| required_caps.contains(cap))
                .count();

            if score > 0 {
                match &best_match {
                    None => best_match = Some((id, profile, score)),
                    Some((_, _, best_score)) if score > *best_score => {
                        best_match = Some((id, profile, score));
                    }
                    _ => {}
                }
            }
        }

        // Return best match or fallback to default
        match best_match {
            Some((id, profile, _)) => Some((id, profile)),
            None => self.get_default_profile(),
        }
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::agent::AgentDefaults;

    fn create_test_config() -> AgentsConfig {
        let mut models = HashMap::new();

        models.insert(
            "coder".to_string(),
            ModelProfile {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                description: Some("Code expert".to_string()),
                capabilities: vec!["code".to_string()],
                temperature: Some(0.3),
                thinking_enabled: None,
                max_tokens: None,
            },
        );

        models.insert(
            "fast".to_string(),
            ModelProfile {
                provider: "zhipu".to_string(),
                model: "glm-4-flash".to_string(),
                description: Some("Fast responses".to_string()),
                capabilities: vec!["fast".to_string()],
                temperature: Some(0.7),
                thinking_enabled: None,
                max_tokens: None,
            },
        );

        AgentsConfig {
            defaults: AgentDefaults {
                model: Some("coder".to_string()),
                ..Default::default()
            },
            models,
        }
    }

    #[test]
    fn test_registry_from_config() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("coder"));
        assert!(registry.contains("fast"));
        assert_eq!(registry.get_default_model_id(), Some("coder"));
    }

    #[test]
    fn test_get_profile() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        let profile = registry.get_profile("coder").unwrap();
        assert_eq!(profile.provider, "openai");
        assert_eq!(profile.model, "gpt-4o");
        assert_eq!(profile.temperature, Some(0.3));
    }

    #[test]
    fn test_missing_profile() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        assert!(registry.get_profile("unknown").is_none());
    }

    #[test]
    fn test_default_profile() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        let (id, profile) = registry.get_default_profile().unwrap();
        assert_eq!(id, "coder");
        assert_eq!(profile.provider, "openai");
    }

    #[test]
    fn test_empty_registry() {
        let registry = ModelRegistry::new();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get_default_model_id().is_none());
        assert!(registry.get_default_profile().is_none());
    }

    #[test]
    fn test_list_available_models() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        let mut models = registry.list_available_models();
        models.sort();

        assert_eq!(models, vec!["coder", "fast"]);
    }

    #[test]
    fn test_get_profile_with_fallback_found() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        // Exact match
        let (id, profile) = registry.get_profile_with_fallback(Some("coder")).unwrap();
        assert_eq!(id, "coder");
        assert_eq!(profile.model, "gpt-4o");
    }

    #[test]
    fn test_get_profile_with_fallback_not_found() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        // Unknown model ID falls back to default
        let (id, profile) = registry.get_profile_with_fallback(Some("unknown")).unwrap();
        assert_eq!(id, "coder"); // Falls back to default
        assert_eq!(profile.model, "gpt-4o");
    }

    #[test]
    fn test_get_profile_with_fallback_none() {
        let config = create_test_config();
        let registry = ModelRegistry::from_config(&config);

        // None returns default
        let (id, _) = registry.get_profile_with_fallback(None).unwrap();
        assert_eq!(id, "coder");
    }

    #[test]
    fn test_get_profile_with_fallback_no_default() {
        let config = AgentsConfig {
            defaults: AgentDefaults {
                model: None,
                ..Default::default()
            },
            models: HashMap::new(),
        };
        let registry = ModelRegistry::from_config(&config);

        // No default available
        assert!(registry.get_profile_with_fallback(None).is_none());
        assert!(registry
            .get_profile_with_fallback(Some("unknown"))
            .is_none());
    }
}

/// Tests for smart model selection feature
#[cfg(feature = "smart-model-selection")]
#[cfg(test)]
mod smart_selection_tests {
    use super::*;
    use crate::config::agent::AgentDefaults;

    fn create_smart_selection_config() -> AgentsConfig {
        let mut models = HashMap::new();

        models.insert(
            "coder".to_string(),
            ModelProfile {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                description: Some("Code expert".to_string()),
                capabilities: vec!["code".to_string(), "reasoning".to_string()],
                temperature: Some(0.3),
                thinking_enabled: None,
                max_tokens: None,
            },
        );

        models.insert(
            "fast".to_string(),
            ModelProfile {
                provider: "zhipu".to_string(),
                model: "glm-4-flash".to_string(),
                description: Some("Fast responses".to_string()),
                capabilities: vec!["fast".to_string()],
                temperature: Some(0.7),
                thinking_enabled: None,
                max_tokens: None,
            },
        );

        models.insert(
            "reasoner".to_string(),
            ModelProfile {
                provider: "anthropic".to_string(),
                model: "claude-3-opus".to_string(),
                description: Some("Deep reasoning".to_string()),
                capabilities: vec!["reasoning".to_string(), "creative".to_string()],
                temperature: Some(0.5),
                thinking_enabled: Some(true),
                max_tokens: None,
            },
        );

        AgentsConfig {
            defaults: AgentDefaults {
                model: Some("coder".to_string()),
                ..Default::default()
            },
            models,
        }
    }

    #[test]
    fn test_analyze_task_capabilities_code() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        let caps = registry.analyze_task_capabilities("Write code to implement a function");
        assert!(caps.contains(&"code".to_string()));
    }

    #[test]
    fn test_analyze_task_capabilities_multiple() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        let caps = registry.analyze_task_capabilities("Debug and analyze this complex code");
        assert!(caps.contains(&"code".to_string()));
        assert!(caps.contains(&"reasoning".to_string()));
    }

    #[test]
    fn test_select_by_capability_code_task() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        let (id, profile) = registry
            .select_by_capability("Implement a new API endpoint")
            .unwrap();
        assert_eq!(id, "coder");
        assert_eq!(profile.model, "gpt-4o");
    }

    #[test]
    fn test_select_by_capability_fast_task() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        let (id, profile) = registry
            .select_by_capability("Give me a quick summary")
            .unwrap();
        assert_eq!(id, "fast");
        assert_eq!(profile.model, "glm-4-flash");
    }

    #[test]
    fn test_select_by_capability_reasoning_task() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        // "reasoning" capability appears in both coder and reasoner
        // reasoner has 2 matching caps (reasoning + creative), coder has 2 (code + reasoning)
        // The task only has reasoning, so either could match
        let result = registry.select_by_capability("Analyze this complex problem");
        assert!(result.is_some());
    }

    #[test]
    fn test_select_by_capability_no_match_uses_default() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        // Task with no recognized keywords
        let (id, _) = registry
            .select_by_capability("Translate this document")
            .unwrap();
        assert_eq!(id, "coder"); // Falls back to default
    }

    #[test]
    fn test_select_by_capability_case_insensitive() {
        let config = create_smart_selection_config();
        let registry = ModelRegistry::from_config(&config);

        let (id, _) = registry
            .select_by_capability("CODE and PROGRAMMING task")
            .unwrap();
        assert_eq!(id, "coder");
    }
}
