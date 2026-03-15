//! Provider configuration schemas
//!
//! LLM provider configuration (OpenAI, OpenRouter, Anthropic, etc.)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Provider type enumeration
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// Built-in provider with known defaults (OpenAI, Anthropic, Gemini, etc.)
    #[default]
    Builtin,
    /// Custom provider with OpenAI or Anthropic compatible API
    Custom,
}

/// API compatibility mode for custom providers
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiCompatibility {
    /// OpenAI-compatible API format
    #[default]
    Openai,
    /// Anthropic-compatible API format
    Anthropic,
}

/// Model-specific configuration including pricing
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    /// Price per million input tokens
    #[serde(default, alias = "priceInputPerMillion")]
    pub price_input_per_million: Option<f64>,

    /// Price per million output tokens
    #[serde(default, alias = "priceOutputPerMillion")]
    pub price_output_per_million: Option<f64>,

    /// Currency code (e.g., "USD", "CNY")
    #[serde(default)]
    pub currency: Option<String>,
}

impl ModelConfig {
    /// Check if this model has complete pricing configuration
    pub fn has_pricing(&self) -> bool {
        self.price_input_per_million.is_some() && self.price_output_per_million.is_some()
    }

    /// Get pricing if complete configuration exists
    pub fn get_pricing(
        &self,
        default_currency: Option<&str>,
    ) -> Option<crate::token_tracker::ModelPricing> {
        match (self.price_input_per_million, self.price_output_per_million) {
            (Some(input), Some(output)) => {
                let currency = self
                    .currency
                    .as_deref()
                    .or(default_currency)
                    .unwrap_or("USD");
                Some(crate::token_tracker::ModelPricing::new(
                    input, output, currency,
                ))
            }
            _ => None,
        }
    }
}

/// Provider configuration (OpenAI, OpenRouter, Anthropic, etc.)
#[derive(Clone, Default)]
pub struct ProviderConfig {
    /// API key for the provider
    pub api_key: Option<String>,

    /// API base URL (for custom endpoints)
    pub api_base: Option<String>,

    /// Whether this provider supports thinking/reasoning mode
    /// (e.g., zhipu/glm-5, deepseek/deepseek-reasoner)
    pub supports_thinking: Option<bool>,

    /// OAuth client ID for providers that support OAuth (e.g., GitHub Copilot)
    pub client_id: Option<String>,

    /// Default currency for model pricing (can be overridden per-model)
    pub default_currency: Option<String>,

    /// Model-specific configurations (including pricing)
    pub models: HashMap<String, ModelConfig>,

    /// Provider type: builtin (default) or custom
    pub provider_type: ProviderType,

    /// API compatibility mode for custom providers (openai or anthropic)
    /// Only relevant when provider_type is Custom
    pub api_compatibility: ApiCompatibility,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "***REDACTED***"))
            .field("api_base", &self.api_base)
            .field("supports_thinking", &self.supports_thinking)
            .field(
                "client_id",
                &self.client_id.as_ref().map(|_| "***REDACTED***"),
            )
            .field("default_currency", &self.default_currency)
            .field("models", &self.models)
            .field("provider_type", &self.provider_type)
            .field("api_compatibility", &self.api_compatibility)
            .finish()
    }
}

impl ProviderConfig {
    /// Check if this provider supports thinking mode.
    pub fn supports_thinking(&self) -> bool {
        self.supports_thinking.unwrap_or(false)
    }

    /// Check if this provider is available (configured and has required credentials).
    ///
    /// Local providers (ollama, litellm) don't require an API key.
    /// Remote providers require a non-empty API key to be configured.
    pub fn is_available(&self, provider_name: &str) -> bool {
        let is_local = matches!(provider_name, "ollama" | "litellm");
        if is_local {
            return true;
        }
        // Check for non-empty API key
        self.api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty())
    }

    /// Get pricing for a specific model.
    ///
    /// Returns the model's pricing configuration, using default_currency as fallback
    /// if the model doesn't specify its own currency.
    pub fn get_pricing_for_model(
        &self,
        model_name: &str,
    ) -> Option<crate::token_tracker::ModelPricing> {
        let model_cfg = self.models.get(model_name)?;
        model_cfg.get_pricing(self.default_currency.as_deref())
    }
}

// ============================================================================
// Backward Compatibility - Legacy Provider Config Parsing
// ============================================================================

/// Legacy provider config for backward compatibility.
///
/// Supports the old format where pricing was at provider level:
/// ```yaml
/// providers:
///   openai:
///     apiKey: sk-xxx
///     priceInputPerMillion: 3.0
///     priceOutputPerMillion: 15.0
///     currency: USD
/// ```
#[derive(Debug, Clone, Deserialize)]
struct LegacyProviderConfig {
    #[serde(default, alias = "apiKey")]
    api_key: Option<String>,

    #[serde(default, alias = "apiBase")]
    api_base: Option<String>,

    #[serde(default, alias = "supportsThinking")]
    supports_thinking: Option<bool>,

    #[serde(default, alias = "clientId")]
    client_id: Option<String>,

    /// New field: default currency for model pricing
    #[serde(default, alias = "defaultCurrency")]
    default_currency: Option<String>,

    /// Legacy: price at provider level (now moved to models)
    #[serde(default, alias = "priceInputPerMillion")]
    price_input_per_million: Option<f64>,

    /// Legacy: price at provider level (now moved to models)
    #[serde(default, alias = "priceOutputPerMillion")]
    price_output_per_million: Option<f64>,

    /// Legacy: currency at provider level (alias for default_currency)
    #[serde(default)]
    currency: Option<String>,

    #[serde(default)]
    models: HashMap<String, ModelConfig>,

    /// Provider type: builtin (default) or custom
    #[serde(default, alias = "type")]
    provider_type: ProviderType,

    /// API compatibility mode for custom providers (openai or anthropic)
    #[serde(default, alias = "apiCompatibility")]
    api_compatibility: ApiCompatibility,
}

impl From<LegacyProviderConfig> for ProviderConfig {
    fn from(legacy: LegacyProviderConfig) -> Self {
        let mut models = legacy.models;

        // Resolve default_currency: prefer explicit default_currency, then currency
        let default_currency = legacy.default_currency.or(legacy.currency.clone());

        // If legacy provider-level pricing exists, create a "_default" entry
        // This allows backward compatibility for get_pricing_for_model
        if let (Some(input), Some(output)) = (
            legacy.price_input_per_million,
            legacy.price_output_per_million,
        ) {
            models
                .entry("_default".to_string())
                .or_insert_with(|| ModelConfig {
                    price_input_per_million: Some(input),
                    price_output_per_million: Some(output),
                    currency: legacy.currency.clone(),
                });
        }

        ProviderConfig {
            api_key: legacy.api_key,
            api_base: legacy.api_base,
            supports_thinking: legacy.supports_thinking,
            client_id: legacy.client_id,
            default_currency,
            models,
            provider_type: legacy.provider_type,
            api_compatibility: legacy.api_compatibility,
        }
    }
}

impl<'de> Deserialize<'de> for ProviderConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let legacy = LegacyProviderConfig::deserialize(deserializer)?;
        Ok(ProviderConfig::from(legacy))
    }
}

impl Serialize for ProviderConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut s = serializer.serialize_struct("ProviderConfig", 8)?;
        s.serialize_field("apiKey", &self.api_key)?;
        s.serialize_field("apiBase", &self.api_base)?;
        s.serialize_field("supportsThinking", &self.supports_thinking)?;
        s.serialize_field("clientId", &self.client_id)?;
        s.serialize_field("defaultCurrency", &self.default_currency)?;
        s.serialize_field("models", &self.models)?;
        s.serialize_field("type", &self.provider_type)?;
        s.serialize_field("apiCompatibility", &self.api_compatibility)?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_has_pricing() {
        let complete = ModelConfig {
            price_input_per_million: Some(1.0),
            price_output_per_million: Some(2.0),
            currency: Some("USD".to_string()),
        };
        assert!(complete.has_pricing());

        let partial = ModelConfig {
            price_input_per_million: Some(1.0),
            price_output_per_million: None,
            currency: None,
        };
        assert!(!partial.has_pricing());
    }

    #[test]
    fn test_model_config_get_pricing() {
        let config = ModelConfig {
            price_input_per_million: Some(1.0),
            price_output_per_million: Some(2.0),
            currency: Some("CNY".to_string()),
        };
        let pricing = config.get_pricing(None).unwrap();
        assert_eq!(pricing.price_input_per_million, 1.0);
        assert_eq!(pricing.price_output_per_million, 2.0);
        assert_eq!(pricing.currency, "CNY");

        // Default currency fallback
        let config = ModelConfig {
            price_input_per_million: Some(1.0),
            price_output_per_million: Some(2.0),
            currency: None,
        };
        let pricing = config.get_pricing(Some("EUR")).unwrap();
        assert_eq!(pricing.currency, "EUR");

        // Ultimate fallback to USD
        let pricing = config.get_pricing(None).unwrap();
        assert_eq!(pricing.currency, "USD");
    }

    #[test]
    fn test_new_format_model_pricing() {
        let yaml = r#"
api_key: sk-xxx
default_currency: CNY
models:
  deepseek-chat:
    price_input_per_million: 0.5
    price_output_per_million: 1.0
  deepseek-reasoner:
    price_input_per_million: 2.0
    price_output_per_million: 8.0
    currency: USD
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();

        // deepseek-chat uses default currency
        let pricing = provider.get_pricing_for_model("deepseek-chat").unwrap();
        assert_eq!(pricing.price_input_per_million, 0.5);
        assert_eq!(pricing.price_output_per_million, 1.0);
        assert_eq!(pricing.currency, "CNY");

        // deepseek-reasoner has its own currency
        let pricing = provider.get_pricing_for_model("deepseek-reasoner").unwrap();
        assert_eq!(pricing.price_input_per_million, 2.0);
        assert_eq!(pricing.price_output_per_million, 8.0);
        assert_eq!(pricing.currency, "USD");

        // Unknown model returns None
        assert!(provider.get_pricing_for_model("unknown").is_none());
    }

    #[test]
    fn test_backward_compatible_provider_pricing() {
        // Old format with provider-level pricing
        let yaml = r#"
api_key: sk-xxx
price_input_per_million: 3.0
price_output_per_million: 15.0
currency: USD
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();

        // Should create a _default entry
        assert!(provider.models.contains_key("_default"));

        // Verify _default model config
        let default_model = provider.models.get("_default").unwrap();
        assert_eq!(default_model.price_input_per_million, Some(3.0));
        assert_eq!(default_model.price_output_per_million, Some(15.0));
        assert_eq!(default_model.currency, Some("USD".to_string()));

        // default_currency should also be set
        assert_eq!(provider.default_currency, Some("USD".to_string()));
    }

    #[test]
    fn test_backward_compatible_with_models() {
        // Old format with both provider-level and model-level pricing
        let yaml = r#"
api_key: sk-xxx
price_input_per_million: 0.5
price_output_per_million: 1.0
currency: CNY
models:
  deepseek-reasoner:
    price_input_per_million: 2.0
    price_output_per_million: 8.0
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();

        // Model-level pricing
        let pricing = provider.get_pricing_for_model("deepseek-reasoner").unwrap();
        assert_eq!(pricing.price_input_per_million, 2.0);
        assert_eq!(pricing.price_output_per_million, 8.0);
        // Uses default_currency since model doesn't specify
        assert_eq!(pricing.currency, "CNY");

        // Provider-level pricing stored in _default
        let default_model = provider.models.get("_default").unwrap();
        assert_eq!(default_model.price_input_per_million, Some(0.5));
        assert_eq!(default_model.price_output_per_million, Some(1.0));
    }

    #[test]
    fn test_camel_case_aliases() {
        let yaml = r#"
apiKey: sk-xxx
apiBase: https://api.example.com
supportsThinking: true
clientId: my-client-id
defaultCurrency: EUR
models:
  gpt-4o:
    priceInputPerMillion: 2.5
    priceOutputPerMillion: 10.0
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(provider.api_key, Some("sk-xxx".to_string()));
        assert_eq!(
            provider.api_base,
            Some("https://api.example.com".to_string())
        );
        assert_eq!(provider.supports_thinking, Some(true));
        assert_eq!(provider.client_id, Some("my-client-id".to_string()));
        assert_eq!(provider.default_currency, Some("EUR".to_string()));

        let pricing = provider.get_pricing_for_model("gpt-4o").unwrap();
        assert_eq!(pricing.price_input_per_million, 2.5);
        assert_eq!(pricing.price_output_per_million, 10.0);
        assert_eq!(pricing.currency, "EUR");
    }

    #[test]
    fn test_provider_is_available() {
        // Remote provider without API key
        let provider = ProviderConfig {
            api_key: None,
            ..Default::default()
        };
        assert!(!provider.is_available("openai"));

        // Remote provider with empty API key
        let provider = ProviderConfig {
            api_key: Some("".to_string()),
            ..Default::default()
        };
        assert!(!provider.is_available("openai"));

        // Remote provider with valid API key
        let provider = ProviderConfig {
            api_key: Some("sk-xxx".to_string()),
            ..Default::default()
        };
        assert!(provider.is_available("openai"));

        // Local provider (ollama) doesn't need API key
        let provider = ProviderConfig::default();
        assert!(provider.is_available("ollama"));

        // Local provider (litellm) doesn't need API key
        assert!(provider.is_available("litellm"));
    }

    #[test]
    fn test_supports_thinking() {
        let provider = ProviderConfig {
            supports_thinking: Some(true),
            ..Default::default()
        };
        assert!(provider.supports_thinking());

        let provider = ProviderConfig {
            supports_thinking: Some(false),
            ..Default::default()
        };
        assert!(!provider.supports_thinking());

        let provider = ProviderConfig::default();
        assert!(!provider.supports_thinking());
    }

    #[test]
    fn test_serialization() {
        let provider = ProviderConfig {
            api_key: Some("sk-xxx".to_string()),
            api_base: None,
            supports_thinking: Some(true),
            client_id: None,
            default_currency: Some("USD".to_string()),
            models: {
                let mut m = HashMap::new();
                m.insert(
                    "gpt-4o".to_string(),
                    ModelConfig {
                        price_input_per_million: Some(2.5),
                        price_output_per_million: Some(10.0),
                        currency: None,
                    },
                );
                m
            },
            provider_type: ProviderType::Builtin,
            api_compatibility: ApiCompatibility::Openai,
        };

        let yaml = serde_yaml::to_string(&provider).unwrap();
        assert!(yaml.contains("apiKey: sk-xxx"));
        assert!(yaml.contains("supportsThinking: true"));
        assert!(yaml.contains("defaultCurrency: USD"));
        assert!(yaml.contains("gpt-4o"));
    }

    #[test]
    fn test_empty_provider() {
        let yaml = "";
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(provider.api_key.is_none());
        assert!(provider.api_base.is_none());
        assert!(provider.supports_thinking.is_none());
        assert!(provider.client_id.is_none());
        assert!(provider.default_currency.is_none());
        assert!(provider.models.is_empty());
        assert_eq!(provider.provider_type, ProviderType::Builtin);
        assert_eq!(provider.api_compatibility, ApiCompatibility::Openai);
    }

    #[test]
    fn test_custom_provider_type() {
        // Test custom provider with OpenAI compatibility
        let yaml = r#"
type: custom
apiCompatibility: openai
api_key: sk-custom
api_base: https://custom.api.com/v1
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(provider.provider_type, ProviderType::Custom);
        assert_eq!(provider.api_compatibility, ApiCompatibility::Openai);
        assert_eq!(provider.api_key, Some("sk-custom".to_string()));
        assert_eq!(
            provider.api_base,
            Some("https://custom.api.com/v1".to_string())
        );
    }

    #[test]
    fn test_custom_provider_anthropic_compatibility() {
        // Test custom provider with Anthropic compatibility
        let yaml = r#"
type: custom
apiCompatibility: anthropic
api_key: sk-ant-custom
api_base: https://anthropic-compatible.api.com/v1
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(provider.provider_type, ProviderType::Custom);
        assert_eq!(provider.api_compatibility, ApiCompatibility::Anthropic);
    }

    #[test]
    fn test_builtin_provider_default() {
        // Builtin is the default
        let yaml = r#"
api_key: sk-xxx
"#;
        let provider: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(provider.provider_type, ProviderType::Builtin);
        assert_eq!(provider.api_compatibility, ApiCompatibility::Openai);
    }

    #[test]
    fn test_provider_type_serialization() {
        assert_eq!(
            serde_yaml::to_string(&ProviderType::Builtin)
                .unwrap()
                .trim(),
            "builtin"
        );
        assert_eq!(
            serde_yaml::to_string(&ProviderType::Custom).unwrap().trim(),
            "custom"
        );
    }

    #[test]
    fn test_api_compatibility_serialization() {
        assert_eq!(
            serde_yaml::to_string(&ApiCompatibility::Openai)
                .unwrap()
                .trim(),
            "openai"
        );
        assert_eq!(
            serde_yaml::to_string(&ApiCompatibility::Anthropic)
                .unwrap()
                .trim(),
            "anthropic"
        );
    }
}
