//! Search index management commands.
//!
//! Note: The built-in Tantivy search has been moved to a standalone MCP server.
//! Use `tantivy-mcp` for advanced full-text search functionality.

use anyhow::Result;
use nanobot_core::config::config_dir;

/// Rebuild search indexes.
///
/// Note: This functionality has been moved to the standalone `tantivy-mcp` server.
pub async fn cmd_search_rebuild(_index_type: &str) -> Result<()> {
    println!("⚠️  Built-in search index management has been deprecated.");
    println!();
    println!("For advanced full-text search, use the standalone tantivy-mcp server:");
    println!("  https://github.com/yeheng/nanobot/tree/main/tantivy-mcp");
    println!();
    println!("The tantivy-mcp server provides:");
    println!("  • JSON document indexing with dynamic schemas");
    println!("  • Full-text search with Tantivy");
    println!("  • MCP protocol support for Claude Code and other clients");
    Ok(())
}

/// Incrementally update search indexes.
///
/// Note: This functionality has been moved to the standalone `tantivy-mcp` server.
pub async fn cmd_search_update(_index_type: &str) -> Result<()> {
    println!("⚠️  Built-in search index management has been deprecated.");
    println!();
    println!("For advanced full-text search, use the standalone tantivy-mcp server:");
    println!("  https://github.com/yeheng/nanobot/tree/main/tantivy-mcp");
    Ok(())
}

/// Show search index status.
pub async fn cmd_search_status() -> Result<()> {
    let config_dir = config_dir();

    println!("📊 Search Index Status\n");

    // Memory index path info
    let memory_index_path = config_dir.join("tantivy-index").join("memory");
    print!("  📁 Memory Index: ");
    if memory_index_path.exists() {
        println!("✅ Directory exists (legacy)");
    } else {
        println!("❌ Not initialized");
    }

    // History index
    let history_index_path = config_dir.join("tantivy-index").join("history");
    print!("  💬 History Index: ");
    if history_index_path.exists() {
        println!("✅ Directory exists (legacy)");
    } else {
        println!("❌ Not initialized");
    }

    // Memory files count
    let memory_dir = config_dir.join("memory");
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

    println!();
    println!("💡 Note: Built-in Tantivy search has been moved to tantivy-mcp.");
    println!("   For advanced full-text search, use the standalone tantivy-mcp server.");

    Ok(())
}
