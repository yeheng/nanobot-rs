//! Embedding configuration
//!
//! Configuration for text embedding models and cache settings.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Embedding configuration for semantic search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Enable embedding-based features (semantic search, history recall)
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Name of the embedding model to use
    #[serde(default = "default_model")]
    pub model: String,

    /// Optional custom cache directory for model weights
    /// If not set, uses default: `~/.gasket/embedding-cache`
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,

    /// Optional path to a pre-downloaded model directory
    /// If set, loads model from this path instead of downloading
    #[serde(default)]
    pub local_model_path: Option<PathBuf>,
}

fn default_enabled() -> bool {
    false
}

fn default_model() -> String {
    "all-MiniLM-L6-v2".to_string()
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: default_model(),
            cache_dir: None,
            local_model_path: None,
        }
    }
}

impl EmbeddingConfig {
    /// Convert to gasket_semantic::EmbeddingConfig
    pub fn to_semantic_config(&self) -> gasket_semantic::EmbeddingConfig {
        let mut config = gasket_semantic::EmbeddingConfig::with_model(&self.model);

        if let Some(ref cache_dir) = self.cache_dir {
            config = config.with_cache_dir(cache_dir.clone());
        }

        if let Some(ref local_path) = self.local_model_path {
            config = config.with_local_model_path(local_path.clone());
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmbeddingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model, "all-MiniLM-L6-v2");
        assert!(config.cache_dir.is_none());
        assert!(config.local_model_path.is_none());
    }

    #[test]
    fn test_parse_minimal_config() {
        let yaml = "";
        let config: EmbeddingConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.model, "all-MiniLM-L6-v2");
    }

    #[test]
    fn test_parse_full_config() {
        let yaml = r#"
enabled: true
model: "BAAI/bge-base-en-v1.5"
cache_dir: "/custom/cache"
local_model_path: "/models/bge-base"
"#;
        let config: EmbeddingConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.model, "BAAI/bge-base-en-v1.5");
        assert_eq!(config.cache_dir, Some(PathBuf::from("/custom/cache")));
        assert_eq!(
            config.local_model_path,
            Some(PathBuf::from("/models/bge-base"))
        );
    }

    #[test]
    fn test_conversion_to_semantic_config() {
        let config = EmbeddingConfig {
            enabled: true,
            model: "BAAI/bge-large-en-v1.5".to_string(),
            cache_dir: Some(PathBuf::from("/tmp/cache")),
            local_model_path: None,
        };

        let semantic_config = config.to_semantic_config();
        assert_eq!(semantic_config.model_name, "BAAI/bge-large-en-v1.5");
        assert_eq!(
            semantic_config.resolve_cache_dir(),
            PathBuf::from("/tmp/cache")
        );
    }
}
