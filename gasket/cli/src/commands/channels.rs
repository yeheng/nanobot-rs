//! Channels 命令实现

use anyhow::{Context, Result};
use colored::Colorize;

use gasket_engine::config::load_config;

/// Show status of all configured channels
#[allow(unused_variables)]
pub async fn cmd_channels_status() -> Result<()> {
    println!("{}\n", "Channel Status".bold());

    let config = load_config()
        .await
        .context("Failed to load configuration")?;

    // Helper function to check if env var is set
    let has_env_credential = |env_var: &str| {
        if env_var.starts_with("${") && env_var.ends_with("}") {
            let var_name = &env_var[2..env_var.len() - 1];
            if std::env::var(var_name).is_ok() {
                return "✓";
            }
        }
        "✗"
    };

    // Helper to check credential (either direct or env var)
    let check_credential = |key: &str| {
        if key.is_empty() {
            "✗"
        } else if key.starts_with("${") {
            has_env_credential(key)
        } else {
            "✓"
        }
    };

    #[allow(unused_mut)]
    let mut has_channels = false;

    // Check Telegram
    #[cfg(feature = "telegram")]
    {
        if let Some(telegram) = &config.channels.telegram {
            has_channels = true;
            let status = if telegram.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let cred = check_credential(&telegram.token);

            println!("{}", "Telegram".cyan().bold());
            println!("  Status:     {}", status);
            println!("  Token:      {}", cred);
            println!("  Allow From: {} users", telegram.allow_from.len());
            println!();
        }
    }

    // Check Discord
    #[cfg(feature = "discord")]
    {
        if let Some(discord) = &config.channels.discord {
            has_channels = true;
            let status = if discord.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let cred = check_credential(&discord.token);

            println!("{}", "Discord".purple().bold());
            println!("  Status:     {}", status);
            println!("  Token:      {}", cred);
            println!("  Allow From: {} users", discord.allow_from.len());
            println!();
        }
    }

    // Check Slack
    #[cfg(feature = "slack")]
    {
        if let Some(slack) = &config.channels.slack {
            has_channels = true;
            let status = if slack.enabled { "enabled" } else { "disabled" };
            let cred = check_credential(&slack.bot_token);

            println!("{}", "Slack".yellow().bold());
            println!("  Status:     {}", status);
            println!("  Bot Token:  {}", cred);
            println!("  Allow From: {} users", slack.allow_from.len());
            println!();
        }
    }

    // Check Feishu
    #[cfg(feature = "feishu")]
    {
        if let Some(feishu) = &config.channels.feishu {
            has_channels = true;
            let status = if feishu.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let cred = check_credential(&feishu.app_id);

            println!("{}", "Feishu".magenta().bold());
            println!("  Status:     {}", status);
            println!("  App ID:     {}", cred);
            println!("  Allow From: {} users", feishu.allow_from.len());
            println!();
        }
    }

    if !has_channels {
        println!("No channels configured.");
        println!("\nAdd channel configuration to ~/.gasket/config.yaml");
        println!("Example:");
        println!(
            r#"
channels:
  telegram:
    enabled: true
    token: "YOUR_BOT_TOKEN"
    allowFrom: []
"#
        );
    }

    // Show compiled features
    println!("{}", "\nCompiled Features:".dimmed());
    println!(
        "  Telegram: {}",
        if cfg!(feature = "telegram") {
            "✓"
        } else {
            "✗"
        }
    );
    println!(
        "  Discord:  {}",
        if cfg!(feature = "discord") {
            "✓"
        } else {
            "✗"
        }
    );
    println!(
        "  Slack:    {}",
        if cfg!(feature = "slack") {
            "✓"
        } else {
            "✗"
        }
    );
    println!(
        "  Feishu:   {}",
        if cfg!(feature = "feishu") {
            "✓"
        } else {
            "✗"
        }
    );

    Ok(())
}
