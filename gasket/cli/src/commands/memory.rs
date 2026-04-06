//! Memory management commands.

use anyhow::Result;
use gasket_engine::agent::memory_manager::MemoryManager;
use gasket_engine::memory::{memory_base_dir, SqliteStore};

/// Rebuild the SQLite metadata index from Markdown files on disk.
///
/// Scans all `.md` files under `~/.gasket/memory/`, re-parses their YAML
/// frontmatter, and replaces the `memory_metadata` table contents.
/// Idempotent — safe to run repeatedly.
pub async fn cmd_memory_reindex() -> Result<()> {
    println!("Reindexing memory store...");

    let base_dir = memory_base_dir();
    if !base_dir.exists() {
        println!("Memory directory does not exist: {}", base_dir.display());
        println!("Nothing to reindex.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let pool = store.pool();

    let manager = MemoryManager::new(base_dir, &pool).await?;
    manager.init().await?;

    let report = manager.reindex().await?;

    println!("Reindex complete:");
    println!("  {} files indexed", report.total_files);
    if report.total_errors > 0 {
        println!(
            "  {} files with broken frontmatter (marked as archived)",
            report.total_errors
        );
    }

    Ok(())
}
