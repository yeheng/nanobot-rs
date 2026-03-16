//! Strongly-typed model identifier
//!
//! Replaces ad-hoc `"provider/model"` string parsing with a single type
//! that is parsed once at the CLI boundary and passed around as a struct.
//!
//! # Parsing Rules
//!
//! - If the string contains `/`, the first segment is the provider, the rest is the model.
//! - If the string does not contain `/`, the entire string is the model (no provider).
//!
//! This simple rule allows custom providers without maintaining a hardcoded list.

use std::fmt;
use std::str::FromStr;

/// A parsed model identifier, optionally qualified with a provider.
///
/// # Examples
///
/// ```
/// use gasket_core::providers::ModelSpec;
///
/// let spec: ModelSpec = "deepseek/deepseek-chat".parse().unwrap();
/// assert_eq!(spec.provider(), Some("deepseek"));
/// assert_eq!(spec.model(), "deepseek-chat");
///
/// let spec: ModelSpec = "gpt-4o".parse().unwrap();
/// assert_eq!(spec.provider(), None);
/// assert_eq!(spec.model(), "gpt-4o");
///
/// // Nested model paths (e.g. openrouter)
/// let spec: ModelSpec = "openrouter/anthropic/claude-sonnet-4".parse().unwrap();
/// assert_eq!(spec.provider(), Some("openrouter"));
/// assert_eq!(spec.model(), "anthropic/claude-sonnet-4");
///
/// // Custom provider
/// let spec: ModelSpec = "my_custom_provider/some-model".parse().unwrap();
/// assert_eq!(spec.provider(), Some("my_custom_provider"));
/// assert_eq!(spec.model(), "some-model");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// The provider id, if explicitly specified.
    provider: Option<String>,
    /// The model name (everything after the provider prefix, or the whole string).
    model: String,
}

impl ModelSpec {
    /// Create a new `ModelSpec` with an explicit provider.
    pub fn with_provider(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            model: model.into(),
        }
    }

    /// Create a `ModelSpec` with only a model name (no explicit provider).
    pub fn model_only(model: impl Into<String>) -> Self {
        Self {
            provider: None,
            model: model.into(),
        }
    }

    /// The provider id, if explicitly given.
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The model name.
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl FromStr for ModelSpec {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Simple rule: if it contains '/', first segment is provider, rest is model
        if let Some(pos) = s.find('/') {
            Ok(Self {
                provider: Some(s[..pos].to_string()),
                model: s[pos + 1..].to_string(),
            })
        } else {
            Ok(Self {
                provider: None,
                model: s.to_string(),
            })
        }
    }
}

impl fmt::Display for ModelSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(p) => write!(f, "{}/{}", p, self.model),
            None => write!(f, "{}", self.model),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_provider_model() {
        let spec: ModelSpec = "deepseek/deepseek-chat".parse().unwrap();
        assert_eq!(spec.provider(), Some("deepseek"));
        assert_eq!(spec.model(), "deepseek-chat");
    }

    #[test]
    fn test_parse_model_only() {
        let spec: ModelSpec = "gpt-4o".parse().unwrap();
        assert_eq!(spec.provider(), None);
        assert_eq!(spec.model(), "gpt-4o");
    }

    #[test]
    fn test_parse_nested_model() {
        let spec: ModelSpec = "openrouter/anthropic/claude-sonnet-4".parse().unwrap();
        assert_eq!(spec.provider(), Some("openrouter"));
        assert_eq!(spec.model(), "anthropic/claude-sonnet-4");
    }

    #[test]
    fn test_custom_provider_parsed_correctly() {
        // This is the key fix: custom providers are now recognized
        let spec: ModelSpec = "custom_provider/my-model".parse().unwrap();
        assert_eq!(spec.provider(), Some("custom_provider"));
        assert_eq!(spec.model(), "my-model");
    }

    #[test]
    fn test_my_custom_provider() {
        // The exact test case from task.md
        let spec: ModelSpec = "my_custom_provider/some-model".parse().unwrap();
        assert_eq!(spec.provider(), Some("my_custom_provider"));
        assert_eq!(spec.model(), "some-model");
    }

    #[test]
    fn test_display_with_provider() {
        let spec = ModelSpec::with_provider("deepseek", "deepseek-chat");
        assert_eq!(spec.to_string(), "deepseek/deepseek-chat");
    }

    #[test]
    fn test_display_model_only() {
        let spec = ModelSpec::model_only("gpt-4o");
        assert_eq!(spec.to_string(), "gpt-4o");
    }

    #[test]
    fn test_constructors() {
        let spec = ModelSpec::with_provider("zhipu", "glm-4");
        assert_eq!(spec.provider(), Some("zhipu"));
        assert_eq!(spec.model(), "glm-4");
    }

    #[test]
    fn test_model_with_slash() {
        // Model names can contain slashes (e.g., anthropic/claude-sonnet-4 via openrouter)
        let spec: ModelSpec = "openrouter/anthropic/claude-sonnet-4".parse().unwrap();
        assert_eq!(spec.provider(), Some("openrouter"));
        assert_eq!(spec.model(), "anthropic/claude-sonnet-4");
    }
}
