//! Status 命令实现

use anyhow::{Context, Result};
use colored::Colorize;

use gasket_engine::config::{load_config, ConfigLoader};

/// Show gasket status
pub async fn cmd_status() -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;

    println!("🐈 gasket status\n");
    println!("Configuration: {:?}", ConfigLoader::new().config_path());

    if config.providers.is_empty() {
        println!("\n{}", "⚠️  No providers configured".yellow());
        println!("Run 'gasket onboard' to get started.");
    } else {
        println!("\nProviders:");
        for (name, provider) in &config.providers {
            let status = if provider.api_key.is_some() {
                "✓".green()
            } else {
                "✗ (no API key)".red()
            };
            println!("  {} {}", name, status);
        }
    }

    Ok(())
}

/// Show authentication status for all providers
pub async fn cmd_auth_status() -> Result<()> {
    println!("{}\n", "Authentication Status".bold());

    let config = load_config().await.context("Failed to load config")?;

    if config.providers.is_empty() {
        println!("No providers configured.");
        println!("\nRun 'gasket auth copilot' to authenticate with GitHub Copilot.");
        return Ok(());
    }

    for (name, provider_config) in &config.providers {
        let status = if name == "copilot" {
            #[cfg(feature = "provider-copilot")]
            {
                if let Some(ref token) = provider_config.api_key {
                    match gasket_engine::providers::CopilotProvider::validate_pat(token).await {
                        Ok(()) => format!("{} Authenticated", "✓".green()),
                        Err(_) => format!("{} Invalid token", "✗".red()),
                    }
                } else {
                    format!("{} Configured (OAuth cached)", "✓".green())
                }
            }
            #[cfg(not(feature = "provider-copilot"))]
            {
                if provider_config.api_key.is_some() {
                    format!("{} Configured (copilot feature disabled)", "✓".green())
                } else {
                    format!("{} No token configured", "✗".red())
                }
            }
        } else if provider_config.api_key.is_some() {
            format!("{} Configured", "✓".green())
        } else {
            format!("{} No API key", "✗".red())
        };

        println!("  {}: {}", name.cyan(), status);
    }

    println!();
    println!("Usage:");
    println!("  gasket auth copilot          # OAuth Device Flow");
    println!("  gasket auth copilot --pat    # Use Personal Access Token");

    Ok(())
}
