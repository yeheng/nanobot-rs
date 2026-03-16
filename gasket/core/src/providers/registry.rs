//! Provider Registry for managing multiple LLM provider instances
//!
//! Provides lazy initialization and caching of provider instances with thread-safe access.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::{debug, info, warn};

use super::common::{OpenAICompatibleProvider, ProviderConfig};
use super::gemini::GeminiProvider;
use super::LlmProvider;
use crate::config::Config;

/// Registry for managing LLM provider instances
///
/// Provides:
/// - Lazy initialization of provider instances
/// - Caching of created instances for reuse
/// - Thread-safe access via RwLock
pub struct ProviderRegistry {
    /// Provider configurations from the config file
    configs: HashMap<String, crate::config::ProviderConfig>,

    /// Cached provider instances
    instances: RwLock<HashMap<String, Arc<dyn LlmProvider>>>,

    /// Default provider name (extracted from agent config)
    default_provider: Option<String>,
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            instances: RwLock::new(HashMap::new()),
            default_provider: None,
        }
    }

    /// Create a registry from the root configuration
    pub fn from_config(config: &Config) -> Self {
        let mut registry = Self::new();

        // Add all provider configurations
        for (name, provider_config) in &config.providers {
            registry
                .configs
                .insert(name.clone(), provider_config.clone());
            debug!("Registered provider config: {}", name);
        }

        // Extract default provider from agent defaults model
        // Format: "provider/model" or just a model ID
        if let Some(ref model) = config.agents.defaults.model {
            let provider_name: Option<&str> = model.split('/').next();
            if let Some(name) = provider_name {
                registry.default_provider = Some(name.to_string());
            }
        }

        registry
    }

    /// Get or create a provider by name
    ///
    /// Returns a cached instance if available, otherwise creates a new one.
    pub fn get_or_create(&self, name: &str) -> anyhow::Result<Arc<dyn LlmProvider>> {
        // Check cache first (read lock)
        {
            let instances = self.instances.read().unwrap();
            if let Some(provider) = instances.get(name) {
                debug!("Using cached provider instance: {}", name);
                return Ok(provider.clone());
            }
        }

        // Create new instance (write lock)
        let provider = self.create_provider(name)?;

        {
            let mut instances = self.instances.write().unwrap();
            instances.insert(name.to_string(), provider.clone());
        }

        info!("Created and cached provider instance: {}", name);
        Ok(provider)
    }

    /// Create a new provider instance
    fn create_provider(&self, name: &str) -> anyhow::Result<Arc<dyn LlmProvider>> {
        let config = self
            .configs
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", name))?;

        // Check if provider is available
        if !config.is_available(name) {
            anyhow::bail!("Provider {} is not available (missing API key)", name);
        }

        // Create provider based on name/type
        let provider: Arc<dyn LlmProvider> = match name {
            "gemini" => {
                let api_key = config
                    .api_key
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Gemini API key not configured"))?;
                Arc::new(GeminiProvider::with_config(
                    api_key.clone(),
                    config.api_base.clone(),
                    None, // Use default model
                    config.proxy_enabled(),
                ))
            }
            _ => {
                // Use OpenAI-compatible provider for most providers
                let api_key = config
                    .api_key
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("API key not configured for {}", name))?;

                let provider_config = ProviderConfig {
                    name: name.to_string(),
                    api_base: config
                        .api_base
                        .clone()
                        .unwrap_or_else(|| Self::get_default_api_base(name)),
                    api_key: api_key.clone(),
                    default_model: Self::get_default_model(name),
                    extra_headers: HashMap::new(),
                    proxy_enabled: config.proxy_enabled(),
                };

                Arc::new(OpenAICompatibleProvider::new(provider_config))
            }
        };

        Ok(provider)
    }

    /// Get default API base URL for known providers
    fn get_default_api_base(name: &str) -> String {
        super::common::get_default_api_base(name)
            .unwrap_or_else(|| format!("https://api.{}.com/v1", name).leak())
            .to_string()
    }

    /// Get default model for known providers
    fn get_default_model(name: &str) -> String {
        super::common::get_default_model(name)
            .unwrap_or("default")
            .to_string()
    }

    /// Check if a provider is configured
    pub fn contains(&self, name: &str) -> bool {
        self.configs.contains_key(name)
    }

    /// Check if a provider is available (configured and has credentials)
    pub fn is_available(&self, name: &str) -> bool {
        self.configs.get(name).is_some_and(|c| c.is_available(name))
    }

    /// List all configured provider names
    pub fn list_providers(&self) -> Vec<&str> {
        self.configs.keys().map(|s| s.as_str()).collect()
    }

    /// List available provider names (configured and have credentials)
    pub fn list_available_providers(&self) -> Vec<&str> {
        self.configs
            .iter()
            .filter(|(name, config)| config.is_available(name))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Get the default provider name
    pub fn get_default_provider(&self) -> Option<&str> {
        self.default_provider.as_deref()
    }

    /// Check if the registry is empty (no providers configured)
    pub fn is_empty(&self) -> bool {
        self.configs.is_empty()
    }

    /// Get the number of configured providers
    pub fn len(&self) -> usize {
        self.configs.len()
    }

    /// Get provider configuration
    pub fn get_config(&self, name: &str) -> Option<&crate::config::ProviderConfig> {
        self.configs.get(name)
    }

    /// Clear cached instances (useful for testing or config reload)
    pub fn clear_cache(&self) {
        let mut instances = self.instances.write().unwrap();
        instances.clear();
        debug!("Cleared provider instance cache");
    }

    /// Log warnings for model profiles that reference unavailable providers
    pub fn validate_model_profiles(&self, registry: &crate::config::ModelRegistry) {
        for model_id in registry.list_available_models() {
            if let Some(profile) = registry.get_profile(model_id) {
                if !self.is_available(&profile.provider) {
                    warn!(
                        "Model profile '{}' references unavailable provider '{}'",
                        model_id, profile.provider
                    );
                }
            }
        }
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_registry() {
        let registry = ProviderRegistry::new();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.list_providers().is_empty());
    }

    #[test]
    fn test_contains_provider() {
        let mut registry = ProviderRegistry::new();
        registry.configs.insert(
            "test".to_string(),
            crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            },
        );

        assert!(registry.contains("test"));
        assert!(!registry.contains("other"));
    }

    #[test]
    fn test_is_available() {
        let mut registry = ProviderRegistry::new();

        // Provider with API key
        registry.configs.insert(
            "openai".to_string(),
            crate::config::ProviderConfig {
                api_key: Some("sk-test".to_string()),
                ..Default::default()
            },
        );

        // Provider without API key
        registry.configs.insert(
            "empty".to_string(),
            crate::config::ProviderConfig {
                api_key: None,
                ..Default::default()
            },
        );

        // Local provider (doesn't need API key)
        registry.configs.insert(
            "ollama".to_string(),
            crate::config::ProviderConfig::default(),
        );

        assert!(registry.is_available("openai"));
        assert!(!registry.is_available("empty"));
        assert!(registry.is_available("ollama")); // Local provider
    }
}
