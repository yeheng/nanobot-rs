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

use anyhow::Result;
use directories::BaseDirs;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use tracing::info;

/// Default embedding model name (fastembed enum variant name, case-insensitive)
pub const DEFAULT_MODEL: &str = "AllMiniLML6V2";

/// Default cache directory name (relative to home directory)
pub const DEFAULT_CACHE_DIR: &str = ".gasket/embedding-cache";

/// Default embedding dimension
pub const DEFAULT_DIMENSION: usize = 384;

// ── Helper Functions ─────────────────────────────────────────────────────

/// Expand tilde (~) in a path to the home directory
fn expand_tilde(path: PathBuf) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~") {
        // Try BaseDirs first
        if let Some(base_dirs) = BaseDirs::new() {
            return base_dirs.home_dir().join(stripped);
        }
        // Fallback to HOME env var
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
        // If all fails, return path without tilde
        return PathBuf::from(stripped);
    }
    path
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
    /// Supported models (use enum variant name, case-insensitive):
    /// - `AllMiniLML6V2` (384-dim, fast, default)
    /// - `AllMiniLML12V2` (384-dim)
    /// - `BGESmallENV15` (384-dim)
    /// - `BGEBaseENV15` (768-dim)
    /// - `BGELargeENV15` (1024-dim)
    /// - `BGEM3` (1024-dim)
    /// - `AllMpnetBaseV2` (768-dim)
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
    /// - If explicitly set, use that (with tilde expansion)
    /// - Otherwise, use default: `~/.gasket/embedding-cache`
    pub fn resolve_cache_dir(&self) -> PathBuf {
        self.cache_dir.clone().map_or_else(
            || {
                // Use default cache directory in home directory
                BaseDirs::new()
                    .map(|dirs| dirs.home_dir().join(DEFAULT_CACHE_DIR))
                    .unwrap_or_else(|| {
                        // Fallback: use HOME env var if BaseDirs fails
                        std::env::var("HOME")
                            .map(|home| PathBuf::from(home).join(DEFAULT_CACHE_DIR))
                            .unwrap_or_else(|_| PathBuf::from(DEFAULT_CACHE_DIR))
                    })
            },
            |path| {
                // Expand tilde (~) in custom cache directory path
                expand_tilde(path)
            },
        )
    }

    /// Get the embedding dimension for the configured model.
    /// Returns dimension from fastembed's EmbeddingModel based on model name.
    /// Falls back to DEFAULT_DIMENSION (384) if model is not recognized.
    pub fn get_model_dimension(&self) -> usize {
        // Try to get dimension from fastembed's built-in model
        if let Ok(embedding_model) = EmbeddingModel::try_from(self.model_name.clone()) {
            if let Ok(info) = TextEmbedding::get_model_info(&embedding_model) {
                return info.dim;
            }
        }

        // Fallback to default dimension for unknown models
        DEFAULT_DIMENSION
    }
}

// ── TextEmbedder Implementation ──────────────────────────────────────────

impl TextEmbedder {
    /// Initialise with the default model (sentence-transformers/all-MiniLM-L6-v2).
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
    /// use gasket_storage::{TextEmbedder, EmbeddingConfig};
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
        // Parse model name to EmbeddingModel enum
        let embedding_model = EmbeddingModel::try_from(model_name.to_string())
            .map_err(|e| anyhow::anyhow!("Unsupported model: '{}'. {}", model_name, e))?;

        let model = TextEmbedding::try_new(
            TextInitOptions::new(embedding_model)
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

        // Try to parse as EmbeddingModel, fallback to default if unrecognized
        let embedding_model = EmbeddingModel::try_from(model_name.to_string())
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

// ── Embedder Trait Implementation ─────────────────────────────────────────

#[async_trait::async_trait]
impl crate::memory::Embedder for TextEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // TextEmbedder uses parking_lot::Mutex internally; the lock is held
        // only for the duration of the ONNX inference which is fast enough
        // to call directly without spawn_blocking.
        self.embed(text)
    }

    fn dimension(&self) -> usize {
        self.dimension
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
        let config = EmbeddingConfig::with_model("BGEBaseENV15");
        assert_eq!(config.model_name, "BGEBaseENV15");
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
        // Test fastembed built-in model dimensions (enum variant names)
        assert_eq!(
            EmbeddingConfig::with_model("AllMiniLML6V2").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("AllMiniLML12V2").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("BGESmallENV15").get_model_dimension(),
            384
        );
        assert_eq!(
            EmbeddingConfig::with_model("BGEBaseENV15").get_model_dimension(),
            768
        );
        assert_eq!(
            EmbeddingConfig::with_model("BGELargeENV15").get_model_dimension(),
            1024
        );
        assert_eq!(
            EmbeddingConfig::with_model("BGEM3").get_model_dimension(),
            1024
        );
    }

    #[test]
    fn test_dimension_priority() {
        // Test fastembed model default dimension (enum variant names)
        let config = EmbeddingConfig::with_model("AllMiniLML6V2");
        assert_eq!(config.get_model_dimension(), 384);

        let config = EmbeddingConfig::with_model("BGEBaseENV15");
        assert_eq!(config.get_model_dimension(), 768);

        let config = EmbeddingConfig::with_model("BGELargeENV15");
        assert_eq!(config.get_model_dimension(), 1024);

        // Default 384 for unknown model
        let config = EmbeddingConfig::with_model("unknown-model");
        assert_eq!(config.get_model_dimension(), 384);
    }
}
