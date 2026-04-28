//! Abstraction over vector storage backends.
//!
//! The `VectorStore` trait decouples the embedding system from any specific
//! storage engine (SQLite, LanceDB, etc.). Each backend is feature-gated so
//! users only pay for the dependencies they actually use.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types shared across all backends
// ---------------------------------------------------------------------------

/// A single embedding record to be stored.
pub struct VectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub session_key: String,
    pub event_type: String,
    pub content_hash: String,
}

/// Result from a vector similarity search.
pub struct SearchResult {
    pub id: String,
    pub score: f32,
}

/// Stored embedding returned by load operations.
pub struct StoredEmbedding {
    pub event_id: String,
    pub session_key: String,
    pub embedding: Vec<f32>,
    pub event_type: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Backend-agnostic vector storage interface.
///
/// Every implementation must be `Send + Sync` so it can be shared across
/// async tasks via `Arc<dyn VectorStore>`.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Upsert a batch of records. Idempotent — duplicate IDs are ignored.
    async fn upsert(&self, records: Vec<VectorRecord>) -> Result<()>;

    /// Approximate nearest-neighbor search.
    ///
    /// Returns up to `top_k` results with score >= `min_score`, sorted by
    /// descending similarity. IDs in `exclude` are skipped.
    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
        exclude: &std::collections::HashSet<String>,
    ) -> Result<Vec<SearchResult>>;

    /// Delete records by ID. Returns the number of records removed.
    async fn delete(&self, ids: &[String]) -> Result<u64>;

    /// Check whether a record with the given ID exists.
    async fn exists(&self, id: &str) -> Result<bool>;

    /// Total number of stored records.
    async fn count(&self) -> Result<i64>;

    /// Return the embedding dimension this store was created with.
    fn dim(&self) -> usize;

    // -- Cold-start helpers ---------------------------------------------------

    /// Load all stored embeddings (for full index rebuild).
    async fn load_all(&self) -> Result<Vec<StoredEmbedding>>;

    /// Load the most recent `limit` embeddings, ordered by created_at DESC.
    async fn load_recent(&self, limit: usize) -> Result<Vec<StoredEmbedding>>;
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for selecting a vector store backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[derive(Default)]
pub enum VectorStoreConfig {
    /// SQLite-backed brute-force search (default, zero extra dependencies).
    #[serde(rename = "SQLite")]
    #[default]
    SQLite,

    /// LanceDB embedded vector database with persistent ANN index.
    #[cfg(feature = "lancedb")]
    LanceDB {
        /// Path to the LanceDB database directory.
        /// e.g. "~/.gasket/vectors"
        path: String,
        /// Table name inside the database. Defaults to "event_embeddings".
        #[serde(default = "default_table_name")]
        table: String,
    },
}

#[cfg(feature = "lancedb")]
fn default_table_name() -> String {
    "event_embeddings".to_string()
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Build a `VectorStore` from configuration.
///
/// - `SQLite` backend requires a `SqlitePool` (passed in from the caller).
/// - `LanceDB` backend opens/creates the database at the configured path.
pub async fn build_vector_store(
    config: &VectorStoreConfig,
    dim: usize,
    sqlite_pool: Option<&sqlx::SqlitePool>,
) -> Result<std::sync::Arc<dyn VectorStore>> {
    match config {
        VectorStoreConfig::SQLite => {
            let pool = sqlite_pool
                .ok_or_else(|| anyhow::anyhow!("SQLite pool required for SQLite backend"))?;
            let store = crate::store::EmbeddingStore::with_dim(pool.clone(), dim);
            store.run_migration().await?;
            Ok(std::sync::Arc::new(store))
        }
        #[cfg(feature = "lancedb")]
        VectorStoreConfig::LanceDB { path, table } => {
            let store = crate::lance_store::LanceVectorStore::open(path, table, dim).await?;
            Ok(std::sync::Arc::new(store))
        }
    }
}
