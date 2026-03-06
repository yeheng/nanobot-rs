//! Search index management commands.

use anyhow::Result;
use nanobot_core::config::config_dir;
use nanobot_core::memory::SqliteStore;
use nanobot_core::search::tantivy::{open_history_index, open_memory_index};

/// Rebuild search indexes.
pub async fn cmd_search_rebuild(index_type: &str) -> Result<()> {
    let config_dir = config_dir();

    match index_type {
        "memory" => rebuild_memory_index(&config_dir).await?,
        "history" => rebuild_history_index(&config_dir).await?,
        "all" => {
            rebuild_memory_index(&config_dir).await?;
            rebuild_history_index(&config_dir).await?;
        }
        _ => {
            anyhow::bail!(
                "Unknown index type: {}. Use 'memory', 'history', or 'all'.",
                index_type
            );
        }
    }

    Ok(())
}

/// Incrementally update search indexes.
pub async fn cmd_search_update(index_type: &str) -> Result<()> {
    let config_dir = config_dir();

    match index_type {
        "memory" => update_memory_index(&config_dir).await?,
        "history" => update_history_index(&config_dir).await?,
        "all" => {
            update_memory_index(&config_dir).await?;
            update_history_index(&config_dir).await?;
        }
        _ => {
            anyhow::bail!(
                "Unknown index type: {}. Use 'memory', 'history', or 'all'.",
                index_type
            );
        }
    }

    Ok(())
}

/// Show search index status.
pub async fn cmd_search_status() -> Result<()> {
    let config_dir = config_dir();

    // Memory index status
    let memory_index_path = config_dir.join("tantivy-index").join("memory");
    let memory_dir = config_dir.join("memory");

    println!("📊 Search Index Status\n");

    // Memory index
    print!("  📁 Memory Index: ");
    if memory_index_path.exists() {
        match open_memory_index(&memory_index_path, &memory_dir) {
            Ok((reader, _)) => {
                let num_docs = reader.num_docs();
                println!("✅ {} documents indexed", num_docs);
            }
            Err(e) => {
                println!("⚠️  Error: {}", e);
            }
        }
    } else {
        println!("❌ Not initialized");
    }

    // History index
    let history_index_path = config_dir.join("tantivy-index").join("history");
    print!("  💬 History Index: ");
    if history_index_path.exists() {
        match open_history_index(&history_index_path) {
            Ok((reader, _)) => {
                let num_docs = reader.num_docs();
                println!("✅ {} messages indexed", num_docs);
            }
            Err(e) => {
                println!("⚠️  Error: {}", e);
            }
        }
    } else {
        println!("❌ Not initialized");
    }

    // Memory files count
    if memory_dir.exists() {
        let file_count = std::fs::read_dir(&memory_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                    .count()
            })
            .unwrap_or(0);
        println!("  📄 Memory Files: {} .md files", file_count);
    } else {
        println!("  📄 Memory Files: Directory not found");
    }

    // Database path info
    let db_path = config_dir.join("nanobot.db");
    if db_path.exists() {
        let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        println!("  🗄️  Database: {} bytes", db_size);
    } else {
        println!("  🗄️  Database: Not found");
    }

    Ok(())
}

async fn rebuild_memory_index(config_dir: &std::path::Path) -> Result<()> {
    let index_path = config_dir.join("tantivy-index").join("memory");
    let memory_dir = config_dir.join("memory");

    println!("🔄 Rebuilding memory index...");

    let (_, mut writer) = open_memory_index(&index_path, &memory_dir)?;
    let count = writer.rebuild().await?;

    println!("✅ Memory index rebuilt: {} documents indexed", count);
    Ok(())
}

async fn rebuild_history_index(config_dir: &std::path::Path) -> Result<()> {
    let index_path = config_dir.join("tantivy-index").join("history");

    println!("🔄 Rebuilding history index...");

    let db = SqliteStore::new().await?;
    let (_, mut writer) = open_history_index(&index_path)?;
    let count = writer.rebuild_from_db(&db).await?;

    println!("✅ History index rebuilt: {} messages indexed", count);
    Ok(())
}

async fn update_memory_index(config_dir: &std::path::Path) -> Result<()> {
    let index_path = config_dir.join("tantivy-index").join("memory");
    let memory_dir = config_dir.join("memory");

    println!("🔄 Updating memory index...");

    let (_, mut writer) = open_memory_index(&index_path, &memory_dir)?;
    let stats = writer.incremental_update().await?;

    println!(
        "✅ Memory index updated: {} added, {} updated, {} removed",
        stats.added, stats.updated, stats.removed
    );
    Ok(())
}

async fn update_history_index(config_dir: &std::path::Path) -> Result<()> {
    let index_path = config_dir.join("tantivy-index").join("history");

    println!("🔄 Updating history index...");

    let db = SqliteStore::new().await?;
    let (_, mut writer) = open_history_index(&index_path)?;
    let stats = writer.incremental_update(&db).await?;

    println!(
        "✅ History index updated: {} added, {} removed",
        stats.added, stats.removed
    );
    Ok(())
}
