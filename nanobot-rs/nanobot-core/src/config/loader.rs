//! Configuration loader
//!
//! Provides loading configuration from YAML files with support for:
//! - Environment variable overrides
//! - Vault placeholder resolution (`{{vault:key}}`)
//!
//! # Vault Integration
//!
//! When configuration values contain `{{vault:key}}` placeholders, the loader
//! automatically resolves them by fetching values from the vault store.
//! This requires:
//! 1. `NANOBOT_VAULT_PASSWORD` environment variable to be set
//! 2. Vault to be unlocked (via `unlock()`)
//!
//! If either condition is not met, the placeholder is left unresolved and a warning is logged.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::fs;
use tracing::{debug, info, warn};

use super::resolver::{VaultPlaceholderResolve, VAULT_PASSWORD_ENV};
use super::schema::Config;
use crate::vault::VaultStore;

/// Get the nanobot config directory
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot")
}

/// Get the config file path
pub fn config_path() -> PathBuf {
    config_dir().join("config.yaml")
}

/// Configuration loader
pub struct ConfigLoader {
    config_dir: PathBuf,
}

impl ConfigLoader {
    /// Create a new config loader
    pub fn new() -> Self {
        Self {
            config_dir: config_dir(),
        }
    }

    /// Create a config loader with a custom directory
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { config_dir: dir }
    }

    /// Get the config file path
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    /// Check if config exists
    pub fn exists(&self) -> bool {
        self.config_path().exists()
    }

    /// Load configuration from file
    ///
    /// This method also resolves vault placeholders (`{{vault:key}}`) if
    /// the vault password is available via `NANOBOT_VAULT_PASSWORD` environment variable.
    pub async fn load(&self) -> Result<Config> {
        let path = self.config_path();

        if !path.exists() {
            info!("Config file not found at {:?}, using defaults", path);
            return Ok(Config::default());
        }

        let content = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let mut config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        // Apply environment variable overrides
        self.apply_env_overrides(&mut config);

        // Resolve vault placeholders
        self.resolve_vault_placeholders(&mut config);

        debug!("Loaded config from {:?}", path);
        Ok(config)
    }

    /// Resolve vault placeholders in configuration
    ///
    /// This method attempts to resolve `{{vault:key}}` placeholders by:
    /// 1. Checking for `NANOBOT_VAULT_PASSWORD` environment variable
    /// 2. Loading and unlocking the vault
    /// 3. Resolving all placeholders in the configuration
    fn resolve_vault_placeholders(&self, config: &mut Config) {
        // Check if vault password is available
        let password = match std::env::var(VAULT_PASSWORD_ENV) {
            Ok(p) if !p.is_empty() => Some(p),
            _ => None,
        };

        let Some(password) = password else {
            debug!("No vault password found, skipping placeholder resolution");
            return;
        };

        // Load vault store
        let mut store = match VaultStore::new() {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to load vault store: {}", e);
                return;
            }
        };

        // Unlock vault
        if let Err(e) = store.unlock(&password) {
            warn!("Failed to unlock vault: {}", e);
            return;
        }

        // Resolve placeholders
        let unresolved = config.resolve_placeholders(&store);

        if !unresolved.is_empty() {
            warn!(
                "Failed to resolve {} vault placeholders: {:?}",
                unresolved.len(),
                unresolved
            );
        } else {
            debug!("All vault placeholders resolved successfully");
        }
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&self, config: &mut Config) {
        // Data-driven env overrides: (ENV_VAR, provider_name, field)
        const ENV_OVERRIDES: &[(&str, &str, bool)] = &[
            // (env_var, provider, is_api_key)
            ("OPENAI_API_KEY", "openai", true),
            ("ANTHROPIC_API_KEY", "anthropic", true),
            ("OPENROUTER_API_KEY", "openrouter", true),
            ("OLLAMA_API_BASE", "ollama", false),
            ("LITELLM_API_BASE", "litellm", false),
            ("LITELLM_API_KEY", "litellm", true),
        ];

        for &(env_var, provider, is_api_key) in ENV_OVERRIDES {
            if let Ok(value) = std::env::var(env_var) {
                let entry = config.providers.entry(provider.to_string()).or_default();
                if is_api_key {
                    entry.api_key = Some(value);
                } else {
                    entry.api_base = Some(value);
                }
            }
        }
    }

    /// Save configuration to file
    pub async fn save(&self, config: &Config) -> Result<()> {
        let path = self.config_path();

        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
        }

        let content = serde_yaml::to_string(config).context("Failed to serialize config")?;

        fs::write(&path, content)
            .await
            .with_context(|| format!("Failed to write config file: {:?}", path))?;

        info!("Saved config to {:?}", path);
        Ok(())
    }

    /// Initialize a default configuration
    pub async fn init_default(&self) -> Result<Config> {
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

/// Load configuration (convenience function)
pub async fn load_config() -> Result<Config> {
    ConfigLoader::new().load().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_loader_new() {
        let loader = ConfigLoader::new();
        assert!(loader.config_path().ends_with("config.yaml"));
    }
}
