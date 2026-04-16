//! Application-level configuration types (YAML config file layer)
//!
//! These types define the **file format** of `~/.gasket/config.yaml`. They are
//! the primary config layer used by the CLI to load, validate, and save
//! configuration.
//!
//! ## Relationship to runtime config types
//!
//! Some types share names with their runtime counterparts in other crates.
//! This is intentional — the file-layer type captures the YAML schema (with
//! optional fields, serde aliases for backward compat), while the runtime type
//! is a focused, non-optional struct used during execution:
//!
//! | File-layer (this module)          | Runtime counterpart              |
//! |----------------------------------|----------------------------------|
//! | `ProviderConfig`                 | `gasket_providers::ProviderConfig` |
//! | `EmbeddingConfig`                | `gasket_storage::EmbeddingConfig`  |
//! | `ChannelsConfig` (re-exported)   | `gasket_channels::ChannelsConfig`  |

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config_dir;
use crate::error::ConfigValidationError;
use crate::token_tracker::ModelPricing;
use crate::vault::contains_placeholders;
use crate::vault::VaultStore;
use crate::vault::{replace_placeholders, scan_placeholders};

use std::env;
use tracing::{debug, warn};

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

/// File-level provider configuration (YAML schema).
///
/// Maps to a single provider entry under `providers:` in `config.yaml`.
/// Converted to `gasket_providers::ProviderConfig` at runtime via
/// `ProviderRegistry::get_or_create`.
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

/// Root configuration structure — maps directly to `~/.gasket/config.yaml`.
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

/// Resolve `{{vault:key}}` placeholders in raw YAML before deserialization.
///
/// Resolution order for each placeholder:
/// 1. **VaultStore** — if the vault file exists and is unlocked
/// 2. **Environment variable** — key uppercased (e.g. `zhipu_api_key` → `ZHIPU_API_KEY`)
///
/// Unresolved placeholders are left as-is in the text and a warning is logged.
/// This prevents accidental `{{vault:...}}` patterns in system prompt text or
/// other non-critical fields from crashing config loading. Critical fields
/// (e.g. `api_key`) are resolved again at JIT time via `ProviderRegistry`.
#[allow(dead_code)] // Available for eager resolution; production uses JIT via ProviderRegistry
fn resolve_config_placeholders(content: &str) -> anyhow::Result<String> {
    let placeholders = scan_placeholders(content);
    if placeholders.is_empty() {
        return Ok(content.to_string());
    }

    debug!(
        "[Config] Found {} vault placeholder(s) in config.yaml",
        placeholders.len()
    );

    // Build replacement map from environment variables only.
    // Vault resolution is intentionally removed from config parsing to avoid
    // triggering expensive Argon2id KDF operations during load_config().
    // Critical fields (api_key) are resolved at JIT time via ProviderRegistry.
    let mut replacements = HashMap::new();
    let mut unresolved = Vec::new();

    for p in &placeholders {
        let env_key = p.key.to_uppercase();
        if let Ok(value) = env::var(&env_key) {
            replacements.insert(p.key.clone(), value);
            debug!("[Config] Resolved '{}' from env var {}", p.key, env_key);
            continue;
        }

        unresolved.push(p.key.clone());
    }

    if !unresolved.is_empty() {
        // Non-fatal: warn and leave unresolved placeholders as-is.
        // Critical fields (api_key) are resolved again at JIT time.
        for key in &unresolved {
            warn!(
                "[Config] Unresolved vault placeholder '{{{{vault:{}}}}}' — \
                 left as-is. Set env {} or unlock vault to resolve.",
                key,
                key.to_uppercase()
            );
        }
    }

    Ok(replace_placeholders(content, &replacements))
}

/// Load configuration from file.
///
/// Vault placeholders (`{{vault:key}}`) are kept as raw strings in the
/// resulting `Config`. They are resolved at the point of use (JIT) via
/// `VaultStore::resolve_text()`.
///
/// **Important:** `{{vault:key}}` values in YAML must be quoted, e.g.
/// `api_key: "{{vault:openai_key}}"`, otherwise YAML parsing will fail.
pub async fn load_config() -> anyhow::Result<Config> {
    let config_path = config_path()?;
    if !config_path.exists() {
        return Ok(Config::default());
    }
    let content = tokio::fs::read_to_string(&config_path).await?;
    let config: Config = serde_yaml::from_str(&content).map_err(|e| {
        // Provide a helpful hint when YAML parsing fails and the raw text
        // contains unquoted vault placeholders.
        if contains_placeholders(&content) {
            anyhow::anyhow!(
                "{e}\n\nHint: vault placeholders must be quoted in YAML.\n\
                 Use  api_key: \"{{{{vault:key}}}}\"  instead of  api_key: {{{{vault:key}}}}"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;
    Ok(config)
}

/// Get the config file path
pub fn config_path() -> std::io::Result<std::path::PathBuf> {
    Ok(config_dir().join("config.yaml"))
}

/// Configuration loader — reads/writes `~/.gasket/config.yaml`.
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
#[derive(Default)]
pub struct ProviderRegistry {
    configs: HashMap<String, ProviderConfig>,
    default_provider: Option<String>,
    vault: Option<std::sync::Arc<VaultStore>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("configs", &self.configs)
            .field("default_provider", &self.default_provider)
            .field("vault", &self.vault.as_ref().map(|_| "VaultStore(..)"))
            .finish()
    }
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

    /// Attach an unlocked vault store for JIT secret resolution.
    pub fn with_vault(&mut self, vault: std::sync::Arc<VaultStore>) {
        self.vault = Some(vault);
    }

    /// Resolve a raw API key string through the vault (JIT).
    ///
    /// - No placeholders → returns the string as-is
    /// - Has placeholders & vault available → resolves
    /// - Has placeholders & no vault → error
    fn resolve_api_key(&self, raw: &str) -> anyhow::Result<String> {
        if !contains_placeholders(raw) {
            return Ok(raw.to_string());
        }

        match self.vault.as_ref() {
            Some(v) => v
                .resolve_text(raw)
                .map_err(|e| anyhow::anyhow!("Vault resolution failed: {}", e)),
            None => anyhow::bail!(
                "Config contains vault placeholder(s) but no vault is available. \
                 Set GASKET_VAULT_PASSWORD or run 'gasket vault unlock'."
            ),
        }
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

        let raw_api_key = config.api_key.as_deref().unwrap_or("");
        let api_key = self.resolve_api_key(raw_api_key)?;

        let provider_config = gasket_providers::ProviderConfig {
            name: name.to_string(),
            api_base: config.api_base.clone(),
            api_key,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Without placeholders, content is returned unchanged.
    #[test]
    fn test_resolve_config_no_placeholders() {
        let yaml = "providers:\n  openai:\n    api_key: sk-123\n";
        let result = resolve_config_placeholders(yaml).unwrap();
        assert_eq!(result, yaml);
    }

    /// Placeholders that have a matching env var are resolved.
    #[test]
    fn test_resolve_config_from_env() {
        env::set_var("GASKET_TEST_ZHIPU_KEY", "sk-from-env");
        let yaml = "api_key: {{vault:gasket_test_zhipu_key}}";
        let result = resolve_config_placeholders(yaml).unwrap();
        env::remove_var("GASKET_TEST_ZHIPU_KEY");
        assert_eq!(result, "api_key: sk-from-env");
    }

    /// Multiple placeholders are resolved in one pass.
    #[test]
    fn test_resolve_config_multiple_from_env() {
        env::set_var("GASKET_TEST_KEY_A", "val-a");
        env::set_var("GASKET_TEST_KEY_B", "val-b");
        let yaml = "a: {{vault:gasket_test_key_a}}\nb: {{vault:gasket_test_key_b}}";
        let result = resolve_config_placeholders(yaml).unwrap();
        env::remove_var("GASKET_TEST_KEY_A");
        env::remove_var("GASKET_TEST_KEY_B");
        assert_eq!(result, "a: val-a\nb: val-b");
    }

    /// Unresolved placeholders no longer produce an error — they are left as-is.
    #[test]
    fn test_resolve_config_unresolved_warns_not_fails() {
        let yaml = "api_key: {{vault:nonexistent_gasket_test_key_xyz}}";
        let result = resolve_config_placeholders(yaml).unwrap();
        // Placeholder is left unchanged, not stripped or replaced
        assert!(result.contains("{{vault:nonexistent_gasket_test_key_xyz}}"));
    }

    /// Placeholders in non-critical fields (e.g. system prompt text) should not
    /// crash config loading — they are simply left as literal text.
    #[test]
    fn test_resolve_config_placeholders_in_prompt_text_resilient() {
        let yaml = r#"
system_prompt: |
  You can reference {{vault:some_key}} but it won't crash if missing.
  This is just documentation text with {{vault:another_missing_key}}.
api_key: "sk-real-key"
"#;
        let result = resolve_config_placeholders(yaml).unwrap();
        // Both unresolved placeholders preserved as literal text
        assert!(result.contains("{{vault:some_key}}"));
        assert!(result.contains("{{vault:another_missing_key}}"));
        // Non-placeholder content is untouched
        assert!(result.contains("sk-real-key"));
    }
}
