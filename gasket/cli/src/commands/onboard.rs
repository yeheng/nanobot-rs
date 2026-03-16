//! Onboard 命令实现

use anyhow::Result;

use gasket_core::config::ConfigLoader;
use gasket_core::workspace::WorkspaceDownloader;

/// Initialize nanobot configuration
pub async fn cmd_onboard() -> Result<()> {
    println!("🐈 Initializing nanobot...\n");

    let loader = ConfigLoader::new();
    let config_path = loader.config_path();
    let workspace = gasket_core::config::config_dir();

    if loader.exists() {
        println!("Configuration already exists at: {:?}", config_path);
        println!("Edit it manually to add your API keys.");
    } else {
        // Create default config
        let _config = loader.init_default().await?;
        println!("Created configuration at: {:?}", config_path);
        println!("\nEdit the config to add your API key:");
        println!("  providers:");
        println!("    openrouter:");
        println!("      apiKey: sk-or-v1-xxx");
    }

    // Download workspace templates from GitHub
    println!("\n📥 Downloading workspace templates from GitHub...");
    let downloader = WorkspaceDownloader::new();
    match downloader.download().await {
        Ok(result) => {
            // Display created files
            for file in &result.created_files {
                println!("  {} ✓", file);
            }
            // Display created directories
            for dir in &result.created_dirs {
                println!("  {}/ ✓", dir);
            }
            // Display skipped files
            for file in &result.skipped_files {
                println!("  {} (already exists, skipped)", file);
            }

            if result.created_files.is_empty()
                && result.created_dirs.is_empty()
                && result.skipped_files.is_empty()
            {
                println!("  (no files to update)");
            }
        }
        Err(e) => {
            println!("⚠️  Download failed: {}", e);
            println!("You may need to manually create workspace files.");
            println!("Templates are available at: https://github.com/yeheng/nanobot-rs/tree/main/workspace");
        }
    }

    println!("\n🐈 Initialization complete!");
    println!("Workspace: {:?}", workspace);

    Ok(())
}
