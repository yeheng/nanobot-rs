//! Embedding-based semantic search for gasket history recall.

pub mod index;
pub mod indexer;
pub mod provider;
pub mod searcher;
pub mod store;

pub use index::HnswIndex;
pub use indexer::EmbeddingIndexer;
pub use provider::{ApiProvider, EmbeddingProvider, ProviderConfig};
pub use searcher::{RecallConfig, RecallHit, RecallSearcher};
pub use store::{EmbeddingStore, StoredEmbedding};
