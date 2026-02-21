//! Strongly-typed model identifier
//!
//! Replaces ad-hoc `"provider/model"` string parsing with a single type
//! that is parsed once at the CLI boundary and passed around as a struct.

use std::fmt;
use std::str::FromStr;

/// Known provider identifiers.
///
/// The first path segment of a model spec is treated as a provider only when
/// it matches one of these names. Otherwise the entire string is the model id.
const KNOWN_PROVIDERS: &[&str] = &[
    "deepseek",
    "openrouter",
    "openai",
    "anthropic",
    "zhipu",
    "dashscope",
    "moonshot",
    "minimax",
    "ollama",
];

/// A parsed model identifier, optionally qualified with a provider.
///
/// # Examples
///
/// ```
/// use nanobot_core::providers::ModelSpec;
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
        if let Some(pos) = s.find('/') {
            let prefix = &s[..pos];
            if KNOWN_PROVIDERS.contains(&prefix) {
                return Ok(Self {
                    provider: Some(prefix.to_string()),
                    model: s[pos + 1..].to_string(),
                });
            }
        }
        Ok(Self {
            provider: None,
            model: s.to_string(),
        })
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
    fn test_unknown_provider_treated_as_model() {
        let spec: ModelSpec = "custom/my-model".parse().unwrap();
        assert_eq!(spec.provider(), None);
        assert_eq!(spec.model(), "custom/my-model");
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
}
