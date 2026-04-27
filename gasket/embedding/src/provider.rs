//! Embedding provider abstraction.

use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
    },
    /// Local ONNX embedding provider (not yet implemented).
    #[cfg(feature = "local-onnx")]
    LocalOnnx { model: String, dim: usize },
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
                dim,
            } => {
                let provider =
                    ApiProvider::new(endpoint.clone(), model.clone(), api_key.clone(), *dim)?;
                Ok(Box::new(provider))
            }
            #[cfg(feature = "local-onnx")]
            ProviderConfig::LocalOnnx { model, dim } => {
                let provider = LocalOnnxProvider::new(model, *dim)?;
                Ok(Box::new(provider))
            }
            #[cfg(not(feature = "local-onnx"))]
            ProviderConfig::LocalOnnx => Err(anyhow!("Local ONNX provider not yet implemented")),
        }
    }
}

/// HTTP-based embedding provider using OpenAI-compatible embeddings API.
pub struct ApiProvider {
    endpoint: String,
    model: String,
    api_key: String,
    dim: usize,
    client: reqwest::Client,
}

impl ApiProvider {
    /// Create a new API provider.
    pub fn new(endpoint: String, model: String, api_key: String, dim: usize) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            endpoint,
            model,
            api_key,
            dim,
            client,
        })
    }

    /// Create a new API provider with a custom reqwest client (for testing).
    pub fn with_client(
        endpoint: String,
        model: String,
        api_key: String,
        dim: usize,
        client: reqwest::Client,
    ) -> Self {
        Self {
            endpoint,
            model,
            api_key,
            dim,
            client,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for ApiProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": [text],
        });

        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("embedding request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("embedding API error {status}: {body}"));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("failed to parse embedding response: {e}"))?;

        // Try OpenAI format first: { data: [{ embedding: [...] }] }
        // Fall back to Ollama native format: { embedding: [...] }
        let embedding = json
            .get("data")
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("embedding"))
            .or_else(|| json.get("embedding"))
            .and_then(|e| e.as_array())
            .ok_or_else(|| anyhow!("unexpected embedding response format: {}", json))?;

        let vec: Vec<f32> = embedding
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| anyhow!("non-numeric embedding value"))
            })
            .collect::<Result<Vec<f32>>>()?;

        Ok(vec)
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("embedding batch request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            // Ollama /api/embeddings does not support batch input — fall back to sequential calls.
            if status.as_u16() == 400 && self.endpoint.contains("/api/embed") {
                tracing::debug!(
                    "Batch not supported by provider, falling back to sequential embed calls"
                );
                let mut results = Vec::with_capacity(texts.len());
                for text in texts {
                    results.push(self.embed(text).await?);
                }
                return Ok(results);
            }
            return Err(anyhow!("embedding API error {status}: {body_text}"));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("failed to parse embedding response: {e}"))?;

        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow!("unexpected embedding response format"))?;

        let mut results = Vec::with_capacity(texts.len());
        for entry in data {
            let embedding = entry
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow!("missing embedding in response entry"))?;
            let vec: Vec<f32> = embedding
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(|f| f as f32)
                        .ok_or_else(|| anyhow!("non-numeric embedding value"))
                })
                .collect::<Result<Vec<f32>>>()?;
            results.push(vec);
        }

        Ok(results)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// Local ONNX embedding provider (fastembed)
// ---------------------------------------------------------------------------

#[cfg(feature = "local-onnx")]
/// Local ONNX embedding provider powered by `fastembed`.
///
/// Models are automatically downloaded from HuggingFace on first use
/// and cached locally. Inference runs in a blocking thread pool so
/// it does not starve the async runtime.
pub struct LocalOnnxProvider {
    model: std::sync::Arc<parking_lot::Mutex<fastembed::TextEmbedding>>,
    dim: usize,
}

#[cfg(feature = "local-onnx")]
impl LocalOnnxProvider {
    /// Create a new local ONNX provider.
    ///
    /// `model_name` is a supported fastembed model name (case-insensitive),
    /// e.g. `"BGESmallENV15"` or `"AllMiniLML6V2"`.
    /// The model is downloaded from HuggingFace on first use if not cached.
    pub fn new(model_name: &str, dim: usize) -> Result<Self> {
        let model_enum: fastembed::EmbeddingModel = model_name
            .parse()
            .map_err(|e: String| anyhow!("unknown local embedding model '{}': {}", model_name, e))?;

        let options = fastembed::TextInitOptions::new(model_enum);
        let model = fastembed::TextEmbedding::try_new(options)
            .map_err(|e| anyhow!("failed to load local embedding model '{}': {}", model_name, e))?;

        Ok(Self {
            model: std::sync::Arc::new(parking_lot::Mutex::new(model)),
            dim,
        })
    }
}

#[cfg(feature = "local-onnx")]
#[async_trait]
impl EmbeddingProvider for LocalOnnxProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let model = self.model.clone();
        let text = text.to_string();
        let embeddings = tokio::task::spawn_blocking(move || {
            let mut model = model.lock();
            model.embed(vec![&text], None)
        })
        .await
        .map_err(|e| anyhow!("embedding task failed: {e}"))?
        .map_err(|e| anyhow!("local embedding failed: {e}"))?;

        embeddings.into_iter().next()
            .ok_or_else(|| anyhow!("local embedding returned no results"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self.model.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let embeddings = tokio::task::spawn_blocking(move || {
            let texts_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let mut model = model.lock();
            model.embed(&texts_refs, None)
        })
        .await
        .map_err(|e| anyhow!("batch embedding task failed: {e}"))?
        .map_err(|e| anyhow!("local batch embedding failed: {e}"))?;

        Ok(embeddings)
    }

    fn dim(&self) -> usize {
        self.dim
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
            } => {
                assert_eq!(endpoint, "https://api.openai.com/v1/embeddings");
                assert_eq!(model, "text-embedding-3-small");
                assert_eq!(api_key, "sk-test-key");
                assert_eq!(dim, 1536);
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
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ProviderConfig = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            ProviderConfig::Api {
                endpoint,
                model,
                api_key,
                dim,
            } => {
                assert_eq!(endpoint, "https://api.example.com/embeddings");
                assert_eq!(model, "model-x");
                assert_eq!(api_key, "key-123");
                assert_eq!(dim, 768);
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
            ProviderConfig::LocalOnnx { model, dim } => {
                assert_eq!(model, "BGESmallENV15");
                assert_eq!(dim, 384);
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
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ProviderConfig = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            ProviderConfig::LocalOnnx { model, dim } => {
                assert_eq!(model, "BGESmallENV15");
                assert_eq!(dim, 384);
            }
            _ => panic!("expected LocalOnnx variant"),
        }
    }

    #[tokio::test]
    async fn test_api_provider_embed_parse_response() {
        // Test that ApiProvider correctly parses a JSON embedding response.
        let mut server = mockito::Server::new_async().await;
        let response_body = serde_json::json!({
            "data": [
                {
                    "embedding": [0.1, 0.2, 0.3],
                    "index": 0
                }
            ],
            "model": "test-model",
            "usage": { "prompt_tokens": 5, "total_tokens": 5 }
        });

        let mock = server
            .mock("POST", "/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_body).unwrap())
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let provider = ApiProvider::with_client(
            format!("{}/embeddings", server.url()),
            "test-model".to_string(),
            "sk-test".to_string(),
            3,
            client,
        );

        let embedding = provider.embed("hello").await.unwrap();
        assert_eq!(embedding, vec![0.1, 0.2, 0.3]);

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_api_provider_embed_batch_parse_response() {
        let mut server = mockito::Server::new_async().await;
        let response_body = serde_json::json!({
            "data": [
                { "embedding": [1.0, 0.0], "index": 0 },
                { "embedding": [0.0, 1.0], "index": 1 }
            ],
            "model": "test-model"
        });

        let mock = server
            .mock("POST", "/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response_body).unwrap())
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let provider = ApiProvider::with_client(
            format!("{}/embeddings", server.url()),
            "test-model".to_string(),
            "sk-test".to_string(),
            2,
            client,
        );

        let embeddings = provider.embed_batch(&["hello", "world"]).await.unwrap();
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], vec![1.0, 0.0]);
        assert_eq!(embeddings[1], vec![0.0, 1.0]);

        mock.assert_async().await;
    }
}
