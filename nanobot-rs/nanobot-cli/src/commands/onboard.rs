//! Onboard 命令实现

use anyhow::Result;

use nanobot_core::config::ConfigLoader;

/// Initialize nanobot configuration
pub async fn cmd_onboard() -> Result<()> {
    println!("🐈 Initializing nanobot...\n");

    let loader = ConfigLoader::new();
    let config_path = loader.config_path();
    let workspace = nanobot_core::config::config_dir();

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

    // Create workspace template files (skip if already exist)
    create_workspace_templates(&workspace)?;

    println!("\n🐈 Initialization complete!");
    println!("Workspace: {:?}", workspace);

    Ok(())
}

/// Create workspace template files under ~/.nanobot/
/// Only creates files that don't already exist (preserves user customizations).
fn create_workspace_templates(workspace: &std::path::Path) -> Result<()> {
    use std::fs;

    // Ensure directories exist
    fs::create_dir_all(workspace.join("memory"))?;
    fs::create_dir_all(workspace.join("skills"))?;

    let templates: &[(&str, &str)] = &[
        (
            "AGENTS.md",
            include_str!("../../../nanobot-core/workspace/AGENTS.md"),
        ),
        (
            "SOUL.md",
            include_str!("../../../nanobot-core/workspace/SOUL.md"),
        ),
        (
            "USER.md",
            include_str!("../../../nanobot-core/workspace/USER.md"),
        ),
        (
            "TOOLS.md",
            include_str!("../../../nanobot-core/workspace/TOOLS.md"),
        ),
        (
            "HEARTBEAT.md",
            include_str!("../../../nanobot-core/workspace/HEARTBEAT.md"),
        ),
    ];

    for (filename, content) in templates {
        let path = workspace.join(filename);
        if path.exists() {
            println!("  {} (already exists, skipped)", filename);
        } else {
            fs::write(&path, content)?;
            println!("  {} ✓", filename);
        }
    }

    Ok(())
}
