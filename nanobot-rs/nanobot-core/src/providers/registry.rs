//! Provider registry for managing LLM providers

use crate::providers::LlmProvider;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// Metadata for a provider
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    /// Provider name
    pub name: String,

    /// API base URL (if applicable)
    pub api_base: Option<String>,

    /// Default model
    pub default_model: String,

    /// Whether this provider is available (has credentials)
    pub available: bool,

    /// Missing configuration (if unavailable)
    pub missing_config: Vec<String>,
}

/// Registry for managing LLM providers
///
/// This is a simplified registry that stores providers by name.
/// Use `ModelSpec` to parse "provider/model" strings and then call `get()`.
pub struct ProviderRegistry {
    /// Registered providers
    providers: HashMap<String, Arc<dyn LlmProvider>>,

    /// Provider metadata
    metadata: HashMap<String, ProviderMetadata>,

    /// Default provider name
    default_provider: Option<String>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            metadata: HashMap::new(),
            default_provider: None,
        }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>, metadata: ProviderMetadata) {
        let name = provider.name().to_string();

        info!(
            "Registering provider: {} (available: {})",
            name, metadata.available
        );

        self.providers.insert(name.clone(), provider);
        self.metadata.insert(name, metadata);
    }

    /// Get a provider by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(name).cloned()
    }

    /// Get provider metadata by name
    pub fn get_metadata(&self, name: &str) -> Option<&ProviderMetadata> {
        self.metadata.get(name)
    }

    /// List all registered providers
    pub fn list_providers(&self) -> Vec<&ProviderMetadata> {
        self.metadata.values().collect()
    }

    /// List available providers (with credentials)
    pub fn list_available(&self) -> Vec<&ProviderMetadata> {
        self.metadata.values().filter(|m| m.available).collect()
    }

    /// Check if a provider is registered
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get total number of providers
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Set the default provider by name.
    ///
    /// The provider must already be registered.
    pub fn set_default(&mut self, name: &str) -> anyhow::Result<()> {
        if !self.providers.contains_key(name) {
            anyhow::bail!("Provider '{}' is not registered", name);
        }
        debug!("Setting default provider to: {}", name);
        self.default_provider = Some(name.to_string());
        Ok(())
    }

    /// Get the default provider.
    ///
    /// Returns the explicitly set default, or falls back to the first
    /// available provider in preference order (openrouter, openai, anthropic).
    pub fn get_default(&self) -> Option<Arc<dyn LlmProvider>> {
        // Explicitly set default
        if let Some(ref name) = self.default_provider {
            if let Some(provider) = self.get(name) {
                return Some(provider);
            }
        }

        // Fallback: prefer providers in standard order
        for default_name in &["openrouter", "deepseek", "openai", "anthropic", "ollama"] {
            if let Some(provider) = self.get(default_name) {
                return Some(provider);
            }
        }

        // Last resort: return any available provider
        for (name, meta) in &self.metadata {
            if meta.available {
                if let Some(provider) = self.providers.get(name) {
                    return Some(provider.clone());
                }
            }
        }

        None
    }

    /// List all registered provider names.
    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatRequest, ChatResponse};
    use anyhow::Result;
    use async_trait::async_trait;

    // Mock provider for testing
    struct MockProvider {
        name: String,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse> {
            Ok(ChatResponse {
                content: Some("mock response".to_string()),
                tool_calls: vec![],
                reasoning_content: None,
            })
        }
    }

    #[test]
    fn test_registry_creation() {
        let registry = ProviderRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_provider() {
        let mut registry = ProviderRegistry::new();

        let provider = Arc::new(MockProvider {
            name: "test".to_string(),
        });

        let metadata = ProviderMetadata {
            name: "test".to_string(),
            api_base: None,
            default_model: "mock-model".to_string(),
            available: true,
            missing_config: vec![],
        };

        registry.register(provider, metadata);

        assert_eq!(registry.len(), 1);
        assert!(registry.contains("test"));
    }

    #[test]
    fn test_get_provider_by_name() {
        let mut registry = ProviderRegistry::new();

        let provider = Arc::new(MockProvider {
            name: "deepseek".to_string(),
        });

        let metadata = ProviderMetadata {
            name: "deepseek".to_string(),
            api_base: Some("https://api.deepseek.com/v1".to_string()),
            default_model: "deepseek-chat".to_string(),
            available: true,
            missing_config: vec![],
        };

        registry.register(provider, metadata);

        // Test basic get by name
        let result = registry.get("deepseek");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name(), "deepseek");

        // Test get non-existent
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_list_available() {
        let mut registry = ProviderRegistry::new();

        // Available provider
        let provider1 = Arc::new(MockProvider {
            name: "available".to_string(),
        });
        let metadata1 = ProviderMetadata {
            name: "available".to_string(),
            api_base: None,
            default_model: "model1".to_string(),
            available: true,
            missing_config: vec![],
        };
        registry.register(provider1, metadata1);

        // Unavailable provider
        let provider2 = Arc::new(MockProvider {
            name: "unavailable".to_string(),
        });
        let metadata2 = ProviderMetadata {
            name: "unavailable".to_string(),
            api_base: None,
            default_model: "model2".to_string(),
            available: false,
            missing_config: vec!["API key not set".to_string()],
        };
        registry.register(provider2, metadata2);

        let available = registry.list_available();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].name, "available");
    }

    #[test]
    fn test_set_and_get_default() {
        let mut registry = ProviderRegistry::new();

        // No default initially
        assert!(registry.get_default().is_none());

        let provider = Arc::new(MockProvider {
            name: "test_provider".to_string(),
        });
        let metadata = ProviderMetadata {
            name: "test_provider".to_string(),
            api_base: None,
            default_model: "mock-model".to_string(),
            available: true,
            missing_config: vec![],
        };
        registry.register(provider, metadata);

        // Set default
        assert!(registry.set_default("test_provider").is_ok());
        let default = registry.get_default();
        assert!(default.is_some());
        assert_eq!(default.unwrap().name(), "test_provider");

        // Setting unregistered default fails
        assert!(registry.set_default("nonexistent").is_err());
    }

    #[test]
    fn test_list_names() {
        let mut registry = ProviderRegistry::new();

        let provider = Arc::new(MockProvider {
            name: "my_provider".to_string(),
        });
        let metadata = ProviderMetadata {
            name: "my_provider".to_string(),
            api_base: None,
            default_model: "m".to_string(),
            available: true,
            missing_config: vec![],
        };
        registry.register(provider, metadata);

        let names = registry.list();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"my_provider"));
    }
}
