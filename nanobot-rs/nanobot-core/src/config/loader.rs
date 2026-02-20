//! Configuration loader

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{debug, info};

use super::schema::Config;

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
    pub fn load(&self) -> Result<Config> {
        let path = self.config_path();

        if !path.exists() {
            info!("Config file not found at {:?}, using defaults", path);
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let mut config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        // Apply environment variable overrides
        self.apply_env_overrides(&mut config);

        debug!("Loaded config from {:?}", path);
        Ok(config)
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&self, config: &mut Config) {
        // Override API keys from environment variables
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            config
                .providers
                .entry("openai".to_string())
                .or_default()
                .api_key = Some(key);
        }

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            config
                .providers
                .entry("anthropic".to_string())
                .or_default()
                .api_key = Some(key);
        }

        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            config
                .providers
                .entry("openrouter".to_string())
                .or_default()
                .api_key = Some(key);
        }
    }

    /// Save configuration to file
    pub fn save(&self, config: &Config) -> Result<()> {
        let path = self.config_path();

        // Create directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
        }

        let content = serde_yaml::to_string(config).context("Failed to serialize config")?;

        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write config file: {:?}", path))?;

        info!("Saved config to {:?}", path);
        Ok(())
    }

    /// Initialize a default configuration
    pub fn init_default(&self) -> Result<Config> {
        let config = Config::default();
        self.save(&config)?;
        Ok(config)
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Load configuration (convenience function)
pub fn load_config() -> Result<Config> {
    ConfigLoader::new().load()
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
