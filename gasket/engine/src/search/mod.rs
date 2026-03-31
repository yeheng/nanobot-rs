//! Search module: re-exports from gasket-storage (absorbed history + semantic)

pub use gasket_storage::search::*;
pub use gasket_storage::{cosine_similarity, top_k_similar};

#[cfg(feature = "local-embedding")]
pub use gasket_storage::{bytes_to_embedding, embedding_to_bytes, TextEmbedder};
