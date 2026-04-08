//! Memory management commands.

use anyhow::Result;
use gasket_engine::agent::MemoryManager;
use gasket_engine::memory::{
    memory_base_dir, AutoIndexHandler, EmbeddingStore, MetadataStore, SqliteStore,
};
#[cfg(feature = "local-embedding")]
use gasket_engine::TextEmbedder;
use gasket_engine::{Embedder, NoopEmbedder};
use tracing::info;
#[cfg(feature = "local-embedding")]
use tracing::warn;

/// Manually refresh memory files from disk, comparing mtime and size to detect changes.
///
/// Only processes files that have changed since the last sync.
/// This is the unified memory refresh command (formerly `reindex` and `refresh`).
pub async fn cmd_memory_refresh() -> Result<()> {
    println!("Refreshing memory files from disk...");

    let base_dir = memory_base_dir();
    if !base_dir.exists() {
        println!("Memory directory does not exist: {}", base_dir.display());
        println!("Nothing to refresh.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let pool = store.pool();

    // Use TextEmbedder if local-embedding feature is enabled, otherwise use NoopEmbedder
    let embedder: Box<dyn Embedder> = {
        #[cfg(feature = "local-embedding")]
        {
            match TextEmbedder::new() {
                Ok(embedder) => {
                    info!("Refresh using TextEmbedder (local-embedding enabled)");
                    Box::new(embedder) as Box<dyn Embedder>
                }
                Err(e) => {
                    warn!(
                        "Failed to initialize TextEmbedder, falling back to NoopEmbedder: {}",
                        e
                    );
                    Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
                }
            }
        }
        #[cfg(not(feature = "local-embedding"))]
        {
            info!("Refresh using NoopEmbedder (local-embedding disabled)");
            Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
        }
    };

    let metadata_store = MetadataStore::new(pool.clone());
    let embedding_store = EmbeddingStore::new(pool.clone());
    let auto_index = AutoIndexHandler::new(
        metadata_store,
        embedding_store,
        base_dir.clone(),
        embedder.into(),
    );

    let report = auto_index.refresh_all_files().await?;

    println!("Refresh complete:");
    println!("  {} files processed", report.processed);
    println!("  {} files updated", report.updated);
    println!("  {} files skipped (unchanged)", report.skipped);
    if report.errors > 0 {
        println!("  {} files with errors", report.errors);
    }

    // Create embedder for MemoryManager
    let embedder: Box<dyn Embedder> = {
        #[cfg(feature = "local-embedding")]
        {
            match TextEmbedder::new() {
                Ok(embedder) => Box::new(embedder) as Box<dyn Embedder>,
                Err(e) => {
                    warn!("Failed to initialize TextEmbedder for MemoryManager: {}", e);
                    Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
                }
            }
        }
        #[cfg(not(feature = "local-embedding"))]
        {
            Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
        }
    };

    let manager = MemoryManager::new(base_dir, &pool, embedder).await?;
    manager.init().await?;

    let report = manager.reindex().await?;

    println!("Reindex complete:");
    println!("  {} files indexed", report.total_files);

    Ok(())
}
