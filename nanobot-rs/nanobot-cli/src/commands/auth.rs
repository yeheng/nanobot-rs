//! Auth 命令实现

use anyhow::Result;
use colored::Colorize;

use nanobot_core::config::ConfigLoader;

/// Login to GitHub Copilot
pub async fn cmd_auth_copilot(pat: Option<String>, client_id: Option<String>) -> Result<()> {
    println!("{}\n", "GitHub Copilot Authentication".bold());

    let loader = ConfigLoader::new();
    let mut config = loader.load().await.unwrap_or_default();

    let access_token = if let Some(token) = pat {
        // PAT mode: validate and use directly
        println!("Validating Personal Access Token...");

        let oauth = nanobot_core::providers::CopilotOAuth::with_default_client_id();
        match oauth.validate_pat(&token).await {
            Ok(true) => {
                println!("{} Token validated successfully", "✓".green());
                token
            }
            Ok(false) => {
                anyhow::bail!(
                    "Invalid Personal Access Token. Ensure it has 'copilot' scope.\n\
                     Create a PAT at: https://github.com/settings/tokens"
                );
            }
            Err(e) => {
                anyhow::bail!("Failed to validate token: {}", e);
            }
        }
    } else {
        // OAuth Device Flow
        let oauth = if let Some(ref cid) = client_id {
            nanobot_core::providers::CopilotOAuth::new(cid)
        } else {
            nanobot_core::providers::CopilotOAuth::with_default_client_id()
        };

        match oauth.start_device_flow().await {
            Ok(token) => {
                println!();
                println!("{} Successfully authenticated!", "✓".green());
                token
            }
            Err(e) => {
                anyhow::bail!(
                    "OAuth authentication failed: {}\n\n\
                     Note: GitHub may restrict OAuth Device Flow for Copilot.\n\
                     Recommended: Use Personal Access Token instead:\n\n\
                     1. Create PAT at: https://github.com/settings/tokens\n\
                     2. Run: nanobot auth copilot --pat <your-token>",
                    e
                );
            }
        }
    };

    // Save to config
    config.providers.insert(
        "copilot".to_string(),
        nanobot_core::config::ProviderConfig {
            api_key: Some(access_token),
            api_base: None,
            supports_thinking: None,
            client_id,
            models: Default::default(),
            default_currency: Some("USD".to_string()),
            provider_type: nanobot_core::config::ProviderType::Builtin,
            api_compatibility: nanobot_core::config::ApiCompatibility::Openai,
        },
    );

    loader.save(&config).await?;
    println!(
        "\n{} Token saved to {:?}",
        "✓".green(),
        loader.config_path()
    );
    println!("\nYou can now use Copilot by setting your model to 'copilot/gpt-4o'");

    Ok(())
}
