//! Embedding provider abstraction.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rig::client::EmbeddingsClient;
use serde::{Deserialize, Serialize};

use crate::rig_adapter::RigEmbeddingAdapter;

/// Trait for embedding providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Compute embedding for a single text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Compute embeddings for multiple texts. Default impl calls `embed` one by one.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Return the embedding dimension.
    fn dim(&self) -> usize;
}

fn default_timeout_secs() -> u64 {
    30
}

/// Configuration for constructing an embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    /// OpenAI-compatible API provider.
    Api {
        endpoint: String,
        model: String,
        api_key: String,
        dim: usize,
        /// HTTP request timeout in seconds (default: 30).
        #[serde(default = "default_timeout_secs")]
        timeout_secs: u64,
    },
    /// Local ONNX embedding provider (powered by `fastembed`).
    #[cfg(feature = "local-onnx")]
    LocalOnnx {
        model: String,
        dim: usize,
        /// Custom directory for downloading/caching the ONNX model files.
        /// Defaults to fastembed's built-in cache path when not set.
        #[serde(default)]
        cache_dir: Option<String>,
    },
    /// Placeholder variant when local-onnx feature is disabled.
    #[cfg(not(feature = "local-onnx"))]
    #[serde(skip)]
    LocalOnnx,
}

impl ProviderConfig {
    /// Construct a boxed provider from this configuration.
    pub fn build(&self) -> Result<Box<dyn EmbeddingProvider>> {
        match self {
            ProviderConfig::Api {
                endpoint,
                model,
                api_key,
                dim: _,
                timeout_secs: _,
            } => {
                // Extract base URL from endpoint by stripping "/embeddings" suffix
                // endpoint is like "https://api.openai.com/v1/embeddings"
                // base URL should be "https://api.openai.com/v1"
                let base_url = endpoint
                    .trim_end_matches("/embeddings")
                    .trim_end_matches("/v1/embeddings")
                    .trim_end_matches("/embeddings");

                let client = rig::providers::openai::Client::builder()
                    .api_key(api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| anyhow!("failed to build rig client: {}", e))?;
                let embedding_model = client.embedding_model(model);
                Ok(Box::new(RigEmbeddingAdapter::new(embedding_model)))
            }
            #[cfg(feature = "local-onnx")]
            ProviderConfig::LocalOnnx {
                model,
                dim,
                cache_dir,
            } => {
                let fastembed_model = FastembedModel::new(model, *dim, cache_dir.as_deref())?;
                Ok(Box::new(RigEmbeddingAdapter::new(fastembed_model)))
            }
            #[cfg(not(feature = "local-onnx"))]
            ProviderConfig::LocalOnnx => Err(anyhow!("Local ONNX provider not yet implemented")),
        }
    }
}


// ---------------------------------------------------------------------------
// Fastembed model adapter implementing rig's EmbeddingModel trait
// ---------------------------------------------------------------------------

#[cfg(feature = "local-onnx")]
/// Local ONNX embedding model powered by `fastembed`, implementing rig's
/// `EmbeddingModel` trait so it can be used through `RigEmbeddingAdapter`.
struct FastembedModel {
    model: std::sync::Arc<parking_lot::Mutex<fastembed::TextEmbedding>>,
    dim: usize,
}

#[cfg(feature = "local-onnx")]
impl FastembedModel {
    fn new(model_name: &str, dim: usize, cache_dir: Option<&str>) -> Result<Self> {
        let model_enum: fastembed::EmbeddingModel = model_name.parse().map_err(|e: String| {
            anyhow!("unknown local embedding model '{}': {}", model_name, e)
        })?;

        let mut options = fastembed::TextInitOptions::new(model_enum);
        if let Some(dir) = cache_dir {
            options = options.with_cache_dir(std::path::PathBuf::from(dir));
        }
        let model = fastembed::TextEmbedding::try_new(options).map_err(|e| {
            anyhow!(
                "failed to load local embedding model '{}': {}",
                model_name,
                e
            )
        })?;

        Ok(Self {
            model: std::sync::Arc::new(parking_lot::Mutex::new(model)),
            dim,
        })
    }
}

#[cfg(feature = "local-onnx")]
impl rig::embeddings::EmbeddingModel for FastembedModel {
    const MAX_DOCUMENTS: usize = 32;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>, _dims: Option<usize>) -> Self {
        unimplemented!("Use FastembedModel::new() to construct")
    }

    fn ndims(&self) -> usize {
        self.dim
    }

    async fn embed_texts(
        &self,
        texts: impl IntoIterator<Item = String> + Send,
    ) -> Result<Vec<rig::embeddings::Embedding>, rig::embeddings::EmbeddingError> {
        let texts: Vec<String> = texts.into_iter().collect();
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self.model.clone();
        let dim = self.dim;
        let result = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let mut model = model.lock();
            model.embed(&refs, None)
        })
        .await
        .map_err(|e| rig::embeddings::EmbeddingError::ProviderError(format!("embedding task failed: {e}")))?
        .map_err(|e| rig::embeddings::EmbeddingError::ProviderError(format!("local embedding failed: {e}")))?;

        if let Some(first) = result.first() {
            if first.len() != dim {
                return Err(rig::embeddings::EmbeddingError::ProviderError(
                    format!("local embedding returned {} dims, expected {}", first.len(), dim)
                ));
            }
        }

        Ok(result
            .into_iter()
            .map(|vec| rig::embeddings::Embedding {
                document: String::new(),
                vec: vec.into_iter().map(|v| v as f64).collect(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Mock provider for testing
// ---------------------------------------------------------------------------

/// Mock provider that returns a fixed-dimension zero vector.
pub struct MockProvider {
    dim: usize,
}

impl MockProvider {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl EmbeddingProvider for MockProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; self.dim])
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider_embed() {
        let provider = MockProvider::new(128);
        let embedding = provider.embed("hello world").await.unwrap();
        assert_eq!(embedding.len(), 128);
        assert_eq!(provider.dim(), 128);
        assert!(embedding.iter().all(|&v| v == 0.0));
    }

    #[tokio::test]
    async fn test_mock_provider_embed_batch() {
        let provider = MockProvider::new(64);
        let texts = vec!["hello", "world", "foo"];
        let embeddings = provider.embed_batch(&texts).await.unwrap();
        assert_eq!(embeddings.len(), 3);
        for emb in &embeddings {
            assert_eq!(emb.len(), 64);
        }
    }

    #[test]
    fn test_provider_config_deserialize_api() {
        let yaml = r#"
type: Api
endpoint: "https://api.openai.com/v1/embeddings"
model: "text-embedding-3-small"
api_key: "sk-test-key"
dim: 1536
"#;
        let config: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        match config {
            ProviderConfig::Api {
                endpoint,
                model,
                api_key,
                dim,
                timeout_secs,
            } => {
                assert_eq!(endpoint, "https://api.openai.com/v1/embeddings");
                assert_eq!(model, "text-embedding-3-small");
                assert_eq!(api_key, "sk-test-key");
                assert_eq!(dim, 1536);
                assert_eq!(timeout_secs, 30, "default timeout should be 30s");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("expected Api variant"),
        }
    }

    #[test]
    fn test_provider_config_build_api() {
        let config = ProviderConfig::Api {
            endpoint: "https://api.openai.com/v1/embeddings".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: "sk-test".to_string(),
            dim: 1536,
            timeout_secs: 30,
        };
        let provider = config.build().unwrap();
        assert_eq!(provider.dim(), 1536);
    }

    #[test]
    fn test_provider_config_serialize_roundtrip() {
        let config = ProviderConfig::Api {
            endpoint: "https://api.example.com/embeddings".to_string(),
            model: "model-x".to_string(),
            api_key: "key-123".to_string(),
            dim: 768,
            timeout_secs: 60,
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ProviderConfig = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            ProviderConfig::Api {
                endpoint,
                model,
                api_key,
                dim,
                timeout_secs,
            } => {
                assert_eq!(endpoint, "https://api.example.com/embeddings");
                assert_eq!(model, "model-x");
                assert_eq!(api_key, "key-123");
                assert_eq!(dim, 768);
                assert_eq!(timeout_secs, 60);
            }
            #[allow(unreachable_patterns)]
            _ => panic!("expected Api variant"),
        }
    }

    #[cfg(feature = "local-onnx")]
    #[test]
    fn test_provider_config_deserialize_local_onnx() {
        let yaml = r#"
type: LocalOnnx
model: BGESmallENV15
dim: 384
"#;
        let config: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        match config {
            ProviderConfig::LocalOnnx {
                model,
                dim,
                cache_dir,
            } => {
                assert_eq!(model, "BGESmallENV15");
                assert_eq!(dim, 384);
                assert_eq!(cache_dir, None);
            }
            _ => panic!("expected LocalOnnx variant"),
        }
    }

    #[cfg(feature = "local-onnx")]
    #[test]
    fn test_provider_config_deserialize_local_onnx_with_cache_dir() {
        let yaml = r#"
type: LocalOnnx
model: BGESmallENV15
dim: 384
cache_dir: "/tmp/gasket-models"
"#;
        let config: ProviderConfig = serde_yaml::from_str(yaml).unwrap();
        match config {
            ProviderConfig::LocalOnnx {
                model,
                dim,
                cache_dir,
            } => {
                assert_eq!(model, "BGESmallENV15");
                assert_eq!(dim, 384);
                assert_eq!(cache_dir.as_deref(), Some("/tmp/gasket-models"));
            }
            _ => panic!("expected LocalOnnx variant"),
        }
    }

    #[cfg(feature = "local-onnx")]
    #[test]
    fn test_provider_config_serialize_roundtrip_local_onnx() {
        let config = ProviderConfig::LocalOnnx {
            model: "BGESmallENV15".to_string(),
            dim: 384,
            cache_dir: None,
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ProviderConfig = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            ProviderConfig::LocalOnnx {
                model,
                dim,
                cache_dir,
            } => {
                assert_eq!(model, "BGESmallENV15");
                assert_eq!(dim, 384);
                assert_eq!(cache_dir, None);
            }
            _ => panic!("expected LocalOnnx variant"),
        }
    }

}
