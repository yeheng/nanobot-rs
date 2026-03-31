//! Offline text embedding engine using fastembed with configurable models.
//!
//! Wraps `fastembed::TextEmbedding` in a `parking_lot::Mutex` because
//! the upstream `embed()` method requires `&mut self` (the tokenizer
//! mutates internal buffers during encoding).
//!
//! Supports:
//! - Multiple embedding models (configurable)
//! - Custom cache directories
//! - Loading pre-downloaded models from local paths

use anyhow::{Context, Result};
use directories::BaseDirs;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use tracing::info;

/// Default embedding model name
pub const DEFAULT_MODEL: &str = "all-MiniLM-L6-v2";

/// Default cache directory name (relative to home directory)
pub const DEFAULT_CACHE_DIR: &str = ".gasket/embedding-cache";

// ── Unified Model Registry (DRY: one source of truth) ───────────────────

/// Model specification: name, dimension, aliases, and fastembed enum.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// Canonical model name (e.g., "all-MiniLM-L6-v2")
    pub name: &'static str,
    /// Embedding dimension
    pub dim: usize,
    /// Optional short alias for convenience (e.g., "minilm")
    pub alias: Option<&'static str>,
    /// FastEmbed enum variant
    pub embedding_model: EmbeddingModel,
}

/// Registry of all supported embedding models.
/// Add new models here — everything else derives from this.
const MODEL_REGISTRY: &[ModelSpec] = &[
    // MiniLM models
    ModelSpec {
        name: "all-MiniLM-L6-v2",
        dim: 384,
        alias: Some("minilm-l6"),
        embedding_model: EmbeddingModel::AllMiniLML6V2,
    },
    ModelSpec {
        name: "all-MiniLM-L12-v2",
        dim: 384,
        alias: Some("minilm-l12"),
        embedding_model: EmbeddingModel::AllMiniLML12V2,
    },
    // BGE English models
    ModelSpec {
        name: "BAAI/bge-small-en-v1.5",
        dim: 384,
        alias: Some("bge-small-en"),
        embedding_model: EmbeddingModel::BGESmallENV15,
    },
    ModelSpec {
        name: "BAAI/bge-base-en-v1.5",
        dim: 768,
        alias: Some("bge-base-en"),
        embedding_model: EmbeddingModel::BGEBaseENV15,
    },
    ModelSpec {
        name: "BAAI/bge-large-en-v1.5",
        dim: 1024,
        alias: Some("bge-large-en"),
        embedding_model: EmbeddingModel::BGELargeENV15,
    },
    // BGE Chinese models
    ModelSpec {
        name: "BAAI/bge-small-zh-v1.5",
        dim: 384,
        alias: Some("bge-small-zh"),
        embedding_model: EmbeddingModel::BGESmallZHV15,
    },
    ModelSpec {
        name: "BAAI/bge-large-zh-v1.5",
        dim: 1024,
        alias: Some("bge-large-zh"),
        embedding_model: EmbeddingModel::BGELargeZHV15,
    },
    // MPNet
    ModelSpec {
        name: "sentence-transformers/all-mpnet-base-v2",
        dim: 768,
        alias: Some("mpnet"),
        embedding_model: EmbeddingModel::AllMpnetBaseV2,
    },
];

/// Find a model spec by name or alias (case-insensitive partial match).
fn find_model_spec(name: &str) -> Option<&'static ModelSpec> {
    // Exact match
    let exact = MODEL_REGISTRY.iter().find(|spec| spec.name == name);
    if exact.is_some() {
        return exact;
    }
    // Alias match
    let alias = MODEL_REGISTRY.iter().find(|spec| spec.alias == Some(name));
    if alias.is_some() {
        return alias;
    }
    // Fuzzy match (e.g., "minilm" matches "all-MiniLM-L6-v2")
    let name_lower = name.to_lowercase();
    MODEL_REGISTRY.iter().find(|spec| {
        spec.name.to_lowercase().contains(&name_lower)
            || spec.alias.map(|a| a.contains(&name_lower)).unwrap_or(false)
    })
}

// ── Configuration ───────────────────────────────────────────────────────

/// Thread-safe text embedder backed by a local ONNX model.
///
/// The model weights are downloaded once from HuggingFace and
/// cached locally.  Subsequent calls run entirely offline in < 10 ms
/// per sentence on modern hardware.
pub struct TextEmbedder {
    model: Mutex<TextEmbedding>,
    config: EmbeddingConfig,
    dimension: usize,
}

/// Configuration for text embedding
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Name of the embedding model to use
    ///
    /// Supported models:
    /// - `all-MiniLM-L6-v2` (384-dim, fast, default)
    /// - `BAAI/bge-small-en-v1.5` (384-dim)
    /// - `BAAI/bge-base-en-v1.5` (768-dim)
    /// - `BAAI/bge-large-en-v1.5` (1024-dim)
    /// - `sentence-transformers/all-mpnet-base-v2` (768-dim)
    pub model_name: String,

    /// Optional custom cache directory for model weights
    /// If None, uses default: `~/.gasket/embedding-cache`
    pub cache_dir: Option<PathBuf>,

    /// Optional path to a pre-downloaded model directory
    /// If set, loads model from this path instead of downloading
    pub local_model_path: Option<PathBuf>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_name: DEFAULT_MODEL.to_string(),
            cache_dir: None,
            local_model_path: None,
        }
    }
}

impl EmbeddingConfig {
    /// Create a new config with the specified model name
    pub fn with_model(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            ..Default::default()
        }
    }

    /// Set the cache directory
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache_dir = Some(cache_dir);
        self
    }

    /// Set the local model path for pre-downloaded models
    pub fn with_local_model_path(mut self, path: PathBuf) -> Self {
        self.local_model_path = Some(path);
        self
    }

    /// Resolve the cache directory path
    /// - If explicitly set, use that
    /// - Otherwise, use default: `~/.gasket/embedding-cache`
    pub fn resolve_cache_dir(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            BaseDirs::new()
                .map(|dirs| dirs.home_dir().join(DEFAULT_CACHE_DIR))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR))
        })
    }

    /// Get the embedding dimension for the configured model.
    /// Returns 384 as fallback for unknown models.
    pub fn get_model_dimension(&self) -> usize {
        find_model_spec(&self.model_name)
            .map(|s| s.dim)
            .unwrap_or_else(|| {
                tracing::warn!(
                    "Unknown model '{}', assuming 384 dimensions. Specify dimension explicitly if needed.",
                    self.model_name
                );
                384
            })
    }
}

// ── TextEmbedder Implementation ──────────────────────────────────────────

impl TextEmbedder {
    /// Initialise with the default model (all-MiniLM-L6-v2).
    ///
    /// **First run only:** downloads model weights (~20-100 MB depending on model).
    pub fn new() -> Result<Self> {
        Self::with_config(EmbeddingConfig::default())
    }

    /// Initialise with a custom configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use gasket_semantic::{TextEmbedder, EmbeddingConfig};
    ///
    /// let config = EmbeddingConfig {
    ///     model_name: "BAAI/bge-base-en-v1.5".to_string(),
    ///     cache_dir: Some("~/.gasket/embedding-cache".into()),
    ///     local_model_path: None,
    /// };
    /// let embedder = TextEmbedder::with_config(config).unwrap();
    /// ```
    pub fn with_config(config: EmbeddingConfig) -> Result<Self> {
        let dimension = config.get_model_dimension();

        info!(
            "Initializing TextEmbedder (model: {}, {}d, cache: {:?})...",
            config.model_name,
            dimension,
            config.resolve_cache_dir()
        );

        let model = if let Some(local_path) = &config.local_model_path {
            // Load from pre-downloaded local path
            info!("Loading model from local path: {:?}", local_path);
            Self::load_model_from_path(local_path)?
        } else {
            // Load from cache (will download if not cached)
            let cache_dir = config.resolve_cache_dir();
            Self::load_model_from_cache(&config.model_name, &cache_dir)?
        };

        info!(
            "TextEmbedder ready (model: {}, {}d)",
            config.model_name, dimension
        );
        Ok(Self {
            model: Mutex::new(model),
            config,
            dimension,
        })
    }

    /// Load a model from the cache directory (downloads if not present)
    fn load_model_from_cache(model_name: &str, cache_dir: &Path) -> Result<TextEmbedding> {
        let spec = find_model_spec(model_name).with_context(|| {
            format!(
                "Unsupported model: '{}'. Supported: {}",
                model_name,
                MODEL_REGISTRY
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

        let model = TextEmbedding::try_new(
            TextInitOptions::new(spec.embedding_model.clone())
                .with_cache_dir(cache_dir.to_path_buf())
                .with_show_download_progress(true),
        )?;
        Ok(model)
    }

    /// Load a model from a pre-downloaded local path.
    ///
    /// The path should contain the ONNX model files:
    /// - model.onnx
    /// - tokenizer.json
    /// - config.json
    fn load_model_from_path(model_path: &Path) -> Result<TextEmbedding> {
        if !model_path.exists() {
            anyhow::bail!("Model path does not exist: {:?}", model_path);
        }

        // Try to guess model type from path name
        let model_name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        info!(
            "Attempting to load model from {:?} (detected name: {})",
            model_path, model_name
        );

        let embedding_model = find_model_spec(model_name)
            .map(|s| s.embedding_model.clone())
            .unwrap_or(EmbeddingModel::AllMiniLML6V2);

        let model = TextEmbedding::try_new(
            TextInitOptions::new(embedding_model)
                .with_cache_dir(model_path.to_path_buf())
                .with_show_download_progress(true),
        )?;
        Ok(model)
    }

    /// Initialise with a pre-downloaded model from a local path.
    ///
    /// # Arguments
    ///
    /// * `model_path` - Path to the directory containing the model files
    /// * `model_name` - Optional model name for dimension lookup (defaults to 384 if not recognized)
    ///
    /// The model path should contain:
    /// - model.onnx
    /// - tokenizer.json  
    /// - config.json
    pub fn with_local_model(model_path: PathBuf, model_name: Option<String>) -> Result<Self> {
        let mut config = EmbeddingConfig::default().with_local_model_path(model_path);
        if let Some(name) = model_name {
            config.model_name = name;
        }
        Self::with_config(config)
    }

    /// Embed a single piece of text into a vector.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut model = self.model.lock();
        let mut embeddings = model.embed(vec![text], None)?;
        embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("TextEmbedder returned empty result"))
    }

    /// Embed multiple texts in a single batch (more efficient than
    /// calling `embed()` in a loop).
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self.model.lock();
        let embeddings = model.embed(texts, None)?;
        Ok(embeddings)
    }

    /// Return the embedding dimension for the loaded model.
    pub const fn dimension(&self) -> usize {
        self.dimension
    }

    /// Return the configuration for this embedder.
    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_dimension() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let vec = embedder.embed("hello world").expect("embed failed");
        assert_eq!(vec.len(), embedder.dimension());
    }

    #[test]
    fn test_embed_batch() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let texts = vec![
            "the weather is nice".to_string(),
            "write some rust code".to_string(),
        ];
        let vecs = embedder.embed_batch(&texts).expect("batch embed failed");
        assert_eq!(vecs.len(), 2);
        for v in &vecs {
            assert_eq!(v.len(), embedder.dimension());
        }
    }

    #[test]
    fn test_embed_empty_batch() {
        let embedder = TextEmbedder::new().expect("Failed to init embedder");
        let vecs = embedder.embed_batch(&[]).expect("empty batch failed");
        assert!(vecs.is_empty());
    }

    #[test]
    fn test_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.model_name, DEFAULT_MODEL);
        assert_eq!(config.get_model_dimension(), 384);
    }

    #[test]
    fn test_config_with_model() {
        let config = EmbeddingConfig::with_model("BAAI/bge-base-en-v1.5");
        assert_eq!(config.model_name, "BAAI/bge-base-en-v1.5");
        assert_eq!(config.get_model_dimension(), 768);
    }

    #[test]
    fn test_config_with_cache_dir() {
        let cache_dir = PathBuf::from("/tmp/test-cache");
        let config = EmbeddingConfig::default().with_cache_dir(cache_dir.clone());
        assert_eq!(config.resolve_cache_dir(), cache_dir);
    }

    #[test]
    fn test_model_dimensions() {
        assert_eq!(
            EmbeddingConfig::with_model("all-MiniLM-L6-v2").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("BAAI/bge-small-en-v1.5").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("BAAI/bge-base-en-v1.5").get_model_dimension(),
            768
        );
        assert_eq!(
            EmbeddingConfig::with_model("BAAI/bge-large-en-v1.5").get_model_dimension(),
            1024
        );
    }

    #[test]
    fn test_model_alias() {
        // Test alias matching
        assert_eq!(
            EmbeddingConfig::with_model("minilm-l6").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("bge-base-en").get_model_dimension(),
            768
        );
    }

    #[test]
    fn test_model_fuzzy_match() {
        // Test fuzzy matching
        assert_eq!(
            EmbeddingConfig::with_model("minilm").get_model_dimension(),
            384
        );
    }

    #[test]
    fn test_find_model_spec() {
        // Exact match
        let spec = find_model_spec("all-MiniLM-L6-v2");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().dim, 384);

        // Alias match
        let spec = find_model_spec("bge-base-en");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().dim, 768);

        // Unknown model
        let spec = find_model_spec("unknown-model");
        assert!(spec.is_none());
    }
}
