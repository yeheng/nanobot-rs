//! Application-level configuration types
//!
//! NOTE: Many of these types are deprecated and will be removed in a future version.
//! Use the individual config types from their respective crates:
//! - Provider config: `gasket_providers::ProviderConfig`
//! - Channel config: `gasket_channels::ChannelsConfig`
//! - Tools config: `gasket_engine::ToolsConfig`

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config_dir;
use crate::error::ConfigValidationError;
use crate::token_tracker::ModelPricing;

// Re-export channel config types
pub use gasket_channels::ChannelsConfig;
// Re-export tools config
use crate::config::ToolsConfig;

/// Embedding configuration (simplified version for config file)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddingConfig {
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[cfg(feature = "local-embedding")]
impl From<EmbeddingConfig> for gasket_storage::EmbeddingConfig {
    fn from(config: EmbeddingConfig) -> Self {
        let mut result = gasket_storage::EmbeddingConfig::default();
        if !config.model_name.is_empty() {
            result.model_name = config.model_name;
        }
        if let Some(dir) = config.cache_dir {
            result.cache_dir = Some(std::path::PathBuf::from(dir));
        }
        result
    }
}

/// Provider API protocol type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    #[default]
    Openai,
    Anthropic,
    Gemini,
}

/// Provider configuration (deprecated - use gasket_providers::ProviderConfig instead)
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub proxy: Option<bool>,
    #[serde(default)]
    pub proxy_enabled: Option<bool>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default, alias = "defaultCurrency")]
    pub default_currency: Option<String>,
}

impl ProviderConfig {
    pub fn is_available(&self, _name: &str) -> bool {
        self.api_key.is_some()
            || self.api_base.contains("localhost")
            || self.api_base.contains("127.0.0.1")
    }

    pub fn proxy_enabled(&self) -> bool {
        self.proxy.unwrap_or(true)
    }

    pub fn thinking_enabled_for_model(&self, model: &str) -> bool {
        self.models
            .get(model)
            .and_then(|m| m.thinking_enabled)
            .unwrap_or(false)
    }

    pub fn get_pricing_for_model(&self, model: &str) -> Option<ModelPricing> {
        self.models.get(model).and_then(|m| {
            match (m.price_input_per_million, m.price_output_per_million) {
                (Some(input), Some(output)) => Some(ModelPricing {
                    price_input_per_million: input,
                    price_output_per_million: output,
                    currency: m.currency.clone().unwrap_or_else(|| "USD".to_string()),
                }),
                _ => None,
            }
        })
    }
}

/// Model-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    #[serde(default, alias = "priceInputPerMillion")]
    pub price_input_per_million: Option<f64>,
    #[serde(default, alias = "priceOutputPerMillion")]
    pub price_output_per_million: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: Option<u32>,
    #[serde(default, alias = "maxIterations")]
    pub max_iterations: Option<u32>,
    #[serde(default, alias = "memoryWindow")]
    pub memory_window: Option<usize>,
    #[serde(default, alias = "thinkingEnabled")]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
}

/// Agent profile for model switching
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelProfile {
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: Option<u32>,
    #[serde(default, alias = "thinkingEnabled")]
    pub thinking_enabled: Option<bool>,
}

/// Agent defaults configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: u32,
    #[serde(default, alias = "maxIterations")]
    pub max_iterations: u32,
    #[serde(default, alias = "memoryWindow")]
    pub memory_window: usize,
    #[serde(default, alias = "thinkingEnabled")]
    pub thinking_enabled: bool,
    #[serde(default)]
    pub streaming: bool,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            model: None,
            temperature: 0.0,
            max_tokens: 0,
            max_iterations: 0,
            memory_window: 0,
            thinking_enabled: false,
            streaming: true,
        }
    }
}

/// Agents configuration section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
}

/// Root configuration structure (deprecated)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub state_machine: Option<serde_json::Value>,
}

impl Config {
    pub fn validate(&self) -> Result<(), Vec<ConfigValidationError>> {
        let mut errors = Vec::new();
        for (name, provider) in &self.providers {
            if !provider.is_available(name) {
                errors.push(ConfigValidationError::ProviderNotAvailable(name.clone()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Load configuration from file
pub async fn load_config() -> anyhow::Result<Config> {
    let config_path = config_path()?;
    if !config_path.exists() {
        return Ok(Config::default());
    }
    let content = tokio::fs::read_to_string(&config_path).await?;
    let config: Config = serde_yaml::from_str(&content)?;
    Ok(config)
}

/// Get the config file path
pub fn config_path() -> std::io::Result<std::path::PathBuf> {
    Ok(config_dir().join("config.yaml"))
}

/// Configuration loader (deprecated)
pub struct ConfigLoader {
    config_path: std::path::PathBuf,
}

impl ConfigLoader {
    pub fn new() -> Self {
        Self {
            config_path: config_path().unwrap_or_else(|_| config_dir().join("config.yaml")),
        }
    }

    pub async fn load(&self) -> anyhow::Result<Config> {
        load_config().await
    }

    pub async fn save(&self, config: &Config) -> anyhow::Result<()> {
        let content = serde_yaml::to_string(config)?;
        tokio::fs::write(&self.config_path, content).await?;
        Ok(())
    }

    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    pub fn exists(&self) -> bool {
        self.config_path.exists()
    }

    pub async fn init_default(&self) -> anyhow::Result<Config> {
        let config = Config::default();
        self.save(&config).await?;
        Ok(config)
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry for managing model profiles
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    profiles: HashMap<String, ModelProfile>,
    default_model_id: Option<String>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(config: &AgentsConfig) -> Self {
        let mut registry = Self::default();
        for (id, profile) in &config.models {
            registry.profiles.insert(id.clone(), profile.clone());
        }
        if let Some(ref model) = config.defaults.model {
            registry.default_model_id = Some(model.clone());
        }
        registry
    }

    pub fn get_profile(&self, id: &str) -> Option<&ModelProfile> {
        self.profiles.get(id)
    }

    pub fn get_profile_with_fallback<'a>(
        &'a self,
        id: Option<&'a str>,
    ) -> Option<(&'a str, &'a ModelProfile)> {
        match id {
            Some(id) => self.profiles.get(id).map(|p| (id, p)),
            None => self.get_default_profile(),
        }
    }

    pub fn get_default_profile(&self) -> Option<(&str, &ModelProfile)> {
        self.default_model_id
            .as_ref()
            .and_then(|id| self.profiles.get(id).map(|p| (id.as_str(), p)))
    }

    pub fn get_default_model_id(&self) -> Option<&str> {
        self.default_model_id.as_deref()
    }

    pub fn list_available_models(&self) -> Vec<&str> {
        self.profiles.keys().map(|s| s.as_str()).collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.profiles.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// Registry for managing LLM provider instances
#[derive(Debug, Default)]
pub struct ProviderRegistry {
    configs: HashMap<String, ProviderConfig>,
    default_provider: Option<String>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(config: &Config) -> Self {
        let mut registry = Self::default();
        for (name, provider_config) in &config.providers {
            registry
                .configs
                .insert(name.clone(), provider_config.clone());
        }
        if let Some(ref model) = config.agents.defaults.model {
            let provider_name: Option<&str> = model.split('/').next();
            if let Some(name) = provider_name {
                registry.default_provider = Some(name.to_string());
            }
        }
        registry
    }

    pub fn get_or_create(
        &self,
        name: &str,
    ) -> anyhow::Result<std::sync::Arc<dyn gasket_providers::LlmProvider>> {
        let config = self
            .configs
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", name))?;

        if !config.is_available(name) {
            anyhow::bail!("Provider {} is not available (missing API key)", name);
        }

        let api_key = config.api_key.as_deref().unwrap_or("");
        let provider_config = gasket_providers::ProviderConfig {
            name: name.to_string(),
            api_base: config.api_base.clone(),
            api_key: api_key.to_string(),
            default_model: "default".to_string(),
            extra_headers: HashMap::new(),
            proxy_enabled: config.proxy_enabled(),
        };

        Ok(std::sync::Arc::new(
            gasket_providers::OpenAICompatibleProvider::new(provider_config),
        ))
    }

    pub fn get_default_provider(&self) -> Option<&str> {
        self.default_provider.as_deref()
    }
}
