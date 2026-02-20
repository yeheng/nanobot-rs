//! Provider registry for managing LLM providers

use crate::providers::LlmProvider;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Metadata for a provider
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    /// Provider name
    pub name: String,

    /// API base URL (if applicable)
    pub api_base: Option<String>,

    /// Default model
    pub default_model: String,

    /// Model name prefix (e.g., "deepseek/", "gemini/")
    pub model_prefix: String,

    /// Whether this provider is available (has credentials)
    pub available: bool,

    /// Missing configuration (if unavailable)
    pub missing_config: Vec<String>,
}

/// Registry for managing LLM providers
pub struct ProviderRegistry {
    /// Registered providers
    providers: HashMap<String, Arc<dyn LlmProvider>>,

    /// Provider metadata
    metadata: HashMap<String, ProviderMetadata>,

    /// Model prefix to provider mapping
    prefix_map: HashMap<String, String>,
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
            prefix_map: HashMap::new(),
        }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>, metadata: ProviderMetadata) {
        let name = provider.name().to_string();
        let prefix = metadata.model_prefix.clone();

        info!(
            "Registering provider: {} (prefix: {}, available: {})",
            name, prefix, metadata.available
        );

        // Add to prefix map
        if !prefix.is_empty() {
            self.prefix_map.insert(prefix.clone(), name.clone());
        }

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

    /// Detect provider from model name
    ///
    /// Examples:
    /// - "deepseek/chat" -> DeepSeek provider
    /// - "gemini/gemini-pro" -> Gemini provider
    /// - "gpt-4" -> Default provider
    pub fn detect_provider(&self, model: &str) -> Option<Arc<dyn LlmProvider>> {
        // Check for prefix (e.g., "deepseek/chat")
        if let Some(slash_pos) = model.find('/') {
            let prefix = &model[..slash_pos];

            if let Some(provider_name) = self.prefix_map.get(prefix) {
                debug!(
                    "Detected provider '{}' from model prefix '{}'",
                    provider_name, prefix
                );
                return self.get(provider_name);
            }
        }

        // No prefix found, try to find a default provider
        // Prefer providers in this order: openrouter, openai, anthropic
        for default_name in &["openrouter", "openai", "anthropic"] {
            if let Some(provider) = self.get(default_name) {
                debug!("Using default provider: {}", default_name);
                return Some(provider);
            }
        }

        warn!("No provider found for model: {}", model);
        None
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

    /// Strip model prefix from model name
    ///
    /// Examples:
    /// - "deepseek/chat" -> "chat"
    /// - "gemini/gemini-pro" -> "gemini-pro"
    /// - "gpt-4" -> "gpt-4"
    pub fn strip_prefix(&self, model: &str) -> String {
        if let Some(slash_pos) = model.find('/') {
            model[slash_pos + 1..].to_string()
        } else {
            model.to_string()
        }
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
            model_prefix: "test".to_string(),
            available: true,
            missing_config: vec![],
        };

        registry.register(provider, metadata);

        assert_eq!(registry.len(), 1);
        assert!(registry.contains("test"));
    }

    #[test]
    fn test_detect_provider_with_prefix() {
        let mut registry = ProviderRegistry::new();

        let provider = Arc::new(MockProvider {
            name: "deepseek".to_string(),
        });

        let metadata = ProviderMetadata {
            name: "deepseek".to_string(),
            api_base: Some("https://api.deepseek.com/v1".to_string()),
            default_model: "deepseek-chat".to_string(),
            model_prefix: "deepseek".to_string(),
            available: true,
            missing_config: vec![],
        };

        registry.register(provider, metadata);

        let detected = registry.detect_provider("deepseek/chat");
        assert!(detected.is_some());
        assert_eq!(detected.unwrap().name(), "deepseek");
    }

    #[test]
    fn test_strip_prefix() {
        let registry = ProviderRegistry::new();

        assert_eq!(registry.strip_prefix("deepseek/chat"), "chat");
        assert_eq!(registry.strip_prefix("gemini/gemini-pro"), "gemini-pro");
        assert_eq!(registry.strip_prefix("gpt-4"), "gpt-4");
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
            model_prefix: "avail".to_string(),
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
            model_prefix: "unavail".to_string(),
            available: false,
            missing_config: vec!["API key not set".to_string()],
        };
        registry.register(provider2, metadata2);

        let available = registry.list_available();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].name, "available");
    }
}
