//! Agent configuration schemas
//!
//! Default agent settings and behavior configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agents configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// Default agent settings
    #[serde(default)]
    pub defaults: AgentDefaults,

    /// Named model profiles for dynamic model switching
    /// Key is the model profile ID (e.g., "coder", "reasoner")
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
}

/// Model profile for dynamic model switching
///
/// Defines a named configuration for a specific model that can be
/// switched to at runtime via the `switch_model` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Provider name (must exist in providers config)
    pub provider: String,

    /// Model identifier for the provider
    pub model: String,

    /// Human-readable description of when to use this model (for LLM guidance)
    #[serde(default)]
    pub description: Option<String>,

    /// Capability tags (e.g., "code", "reasoning", "creative", "fast")
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Temperature override (optional, uses agent default if not set)
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Enable thinking/reasoning mode
    #[serde(default)]
    pub thinking_enabled: Option<bool>,

    /// Max tokens override
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

impl ModelProfile {
    /// Validate the model profile configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.provider.trim().is_empty() {
            return Err("provider cannot be empty".to_string());
        }
        if self.model.trim().is_empty() {
            return Err("model cannot be empty".to_string());
        }
        if let Some(temp) = self.temperature {
            if !(0.0..=2.0).contains(&temp) {
                return Err(format!(
                    "temperature must be between 0.0 and 2.0, got {}",
                    temp
                ));
            }
        }
        if let Some(tokens) = self.max_tokens {
            if tokens == 0 {
                return Err("max_tokens must be greater than 0".to_string());
            }
        }
        Ok(())
    }
}

/// Default agent settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    /// Model to use
    #[serde(default)]
    pub model: Option<String>,

    /// Temperature for generation
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Maximum tool call iterations
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Memory window size
    #[serde(default = "default_memory_window")]
    pub memory_window: usize,

    /// Enable thinking/reasoning mode for deep reasoning models (GLM-5, DeepSeek R1, etc.)
    #[serde(default)]
    pub thinking_enabled: bool,

    /// Enable streaming mode for progressive output (default: true)
    #[serde(default = "default_streaming")]
    pub streaming: bool,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            model: None,
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            max_iterations: default_max_iterations(),
            memory_window: default_memory_window(),
            thinking_enabled: false,
            streaming: default_streaming(),
        }
    }
}

// Default value functions
fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_max_iterations() -> u32 {
    20
}
fn default_memory_window() -> usize {
    50
}
fn default_streaming() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_defaults() {
        let yaml = r#"
defaults:
  model: anthropic/claude-opus-4-5
  temperature: 0.5
  max_tokens: 8192
"#;
        let agents: AgentsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            agents.defaults.model,
            Some("anthropic/claude-opus-4-5".to_string())
        );
        assert_eq!(agents.defaults.temperature, 0.5);
        assert_eq!(agents.defaults.max_tokens, 8192);
        // Default values
        assert_eq!(agents.defaults.max_iterations, 20);
        assert_eq!(agents.defaults.memory_window, 50);
        assert!(agents.defaults.streaming);
        assert!(!agents.defaults.thinking_enabled);
    }

    #[test]
    fn test_agent_defaults_empty() {
        let yaml = "";
        let agents: AgentsConfig = serde_yaml::from_str(yaml).unwrap();
        // All defaults
        assert!(agents.defaults.model.is_none());
        assert_eq!(agents.defaults.temperature, 0.7);
        assert_eq!(agents.defaults.max_tokens, 4096);
    }

    #[test]
    fn test_model_profile_parsing() {
        let yaml = r#"
defaults:
  model: zhipu/glm-5
models:
  coder:
    provider: openai
    model: gpt-4o
    description: "Best for code generation and debugging"
    capabilities:
      - code
      - reasoning
    temperature: 0.3
  reasoner:
    provider: openrouter
    model: anthropic/claude-opus-4
    description: "Deep reasoning for complex analysis"
    capabilities:
      - reasoning
      - creative
    thinking_enabled: true
"#;
        let agents: AgentsConfig = serde_yaml::from_str(yaml).unwrap();

        // Check models map
        assert_eq!(agents.models.len(), 2);

        // Check coder profile
        let coder = agents.models.get("coder").unwrap();
        assert_eq!(coder.provider, "openai");
        assert_eq!(coder.model, "gpt-4o");
        assert_eq!(
            coder.description,
            Some("Best for code generation and debugging".to_string())
        );
        assert_eq!(coder.capabilities, vec!["code", "reasoning"]);
        assert_eq!(coder.temperature, Some(0.3));
        assert_eq!(coder.thinking_enabled, None);

        // Check reasoner profile
        let reasoner = agents.models.get("reasoner").unwrap();
        assert_eq!(reasoner.provider, "openrouter");
        assert_eq!(reasoner.model, "anthropic/claude-opus-4");
        assert_eq!(reasoner.thinking_enabled, Some(true));
    }

    #[test]
    fn test_model_profile_validation() {
        // Valid profile
        let valid = ModelProfile {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            description: None,
            capabilities: vec![],
            temperature: Some(0.5),
            thinking_enabled: None,
            max_tokens: Some(4096),
        };
        assert!(valid.validate().is_ok());

        // Empty provider
        let invalid_provider = ModelProfile {
            provider: "".to_string(),
            model: "gpt-4o".to_string(),
            description: None,
            capabilities: vec![],
            temperature: None,
            thinking_enabled: None,
            max_tokens: None,
        };
        assert!(invalid_provider.validate().is_err());

        // Invalid temperature
        let invalid_temp = ModelProfile {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            description: None,
            capabilities: vec![],
            temperature: Some(3.0),
            thinking_enabled: None,
            max_tokens: None,
        };
        assert!(invalid_temp.validate().is_err());
    }

    #[test]
    fn test_models_empty_by_default() {
        let yaml = r#"
defaults:
  model: test-model
"#;
        let agents: AgentsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(agents.models.is_empty());
    }
}
