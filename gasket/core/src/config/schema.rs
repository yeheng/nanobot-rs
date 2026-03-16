//! Configuration schema
//!
//! Root configuration structure that composes all sub-configurations.
//!
//! Compatible with Python nanobot's config format (now uses YAML)

use crate::error::ConfigValidationError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Re-export from submodules
pub use super::agent::AgentsConfig;
pub use super::channel::{
    ChannelsConfig, DingTalkConfig, DiscordConfig, EmailConfig, FeishuConfig, SlackConfig,
    TelegramConfig,
};
pub use super::provider::{ModelConfig, ProviderConfig};
pub use super::tools::{
    CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig, ToolsConfig,
    WebToolsConfig,
};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// LLM providers configuration
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Agent configuration
    #[serde(default)]
    pub agents: AgentsConfig,

    /// Channel configurations
    #[serde(default)]
    pub channels: ChannelsConfig,

    /// Tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Multi-agent state machine configuration (opt-in).
    /// When absent or `enabled: false`, the state machine subsystem is completely dormant.
    #[serde(default)]
    pub state_machine: Option<serde_json::Value>,
}

impl Config {
    /// Validate the entire configuration
    pub fn validate(&self) -> Result<(), Vec<ConfigValidationError>> {
        let mut errors = Vec::new();

        // Validate providers
        for (name, provider) in &self.providers {
            if !provider.is_available(name) {
                errors.push(ConfigValidationError::ProviderNotAvailable(name.clone()));
            }
        }

        // Validate channels
        errors.extend(self.channels.validate());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_config() {
        let yaml = "";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_parse_provider_config() {
        let yaml = r#"
providers:
  openrouter:
    api_key: sk-or-v1-xxx
agents:
  defaults:
    model: anthropic/claude-opus-4-5
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.providers.get("openrouter").unwrap().api_key,
            Some("sk-or-v1-xxx".to_string())
        );
        assert_eq!(
            config.agents.defaults.model,
            Some("anthropic/claude-opus-4-5".to_string())
        );
    }

    #[test]
    fn test_config_validate_provider() {
        // Provider without API key should fail
        let yaml = r#"
providers:
  openai:
    api_key: ""
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            ConfigValidationError::ProviderNotAvailable(_)
        ));

        // Local provider (ollama) doesn't need API key
        let yaml = r#"
providers:
  ollama:
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_validate_email() {
        let yaml = r#"
channels:
  email:
    enabled: true
    imapHost: imap.example.com
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(matches!(
            errors[0],
            ConfigValidationError::IncompleteEmailConfig
        ));
    }
}
