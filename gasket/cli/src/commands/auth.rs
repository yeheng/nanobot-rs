//! Auth 命令实现

use anyhow::Result;
#[cfg(feature = "provider-copilot")]
use colored::Colorize;

#[cfg(feature = "provider-copilot")]
use gasket_engine::config::ConfigLoader;

/// GitHub Copilot API base URL
#[cfg(feature = "provider-copilot")]
const COPILOT_API_BASE: &str = "https://api.githubcopilot.com";

/// Login to GitHub Copilot
#[cfg(feature = "provider-copilot")]
pub async fn cmd_auth_copilot(pat: Option<String>, client_id: Option<String>) -> Result<()> {
    println!("{}\n", "GitHub Copilot Authentication".bold());

    let loader = ConfigLoader::new();
    let mut config = loader.load().await.unwrap_or_default();

    // Preserve existing proxy configuration if present
    let existing_proxy = config.providers.get("copilot").map(|p| {
        (
            p.proxy_url.clone(),
            p.proxy_username.clone(),
            p.proxy_password.clone(),
        )
    });
    let (proxy_url, proxy_username, proxy_password) = existing_proxy
        .unwrap_or((None, None, None));

    let access_token = if let Some(token) = pat {
        // PAT mode: validate and use directly
        println!("Validating Personal Access Token...");

        let oauth = gasket_engine::providers::CopilotOAuth::with_proxy(
            gasket_engine::providers::COPILOT_DEFAULT_CLIENT_ID,
            proxy_url.clone(),
            proxy_username.clone(),
            proxy_password.clone(),
        );
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
            gasket_engine::providers::CopilotOAuth::with_proxy(
                cid,
                proxy_url.clone(),
                proxy_username.clone(),
                proxy_password.clone(),
            )
        } else {
            gasket_engine::providers::CopilotOAuth::with_proxy(
                gasket_engine::providers::COPILOT_DEFAULT_CLIENT_ID,
                proxy_url.clone(),
                proxy_username.clone(),
                proxy_password.clone(),
            )
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
                     2. Run: gasket auth copilot --pat <your-token>",
                    e
                );
            }
        }
    };

    // Save to config with new explicit format
    config.providers.insert(
        "copilot".to_string(),
        gasket_engine::config::ProviderConfig {
            provider_type: gasket_engine::config::ProviderType::Openai,
            api_base: COPILOT_API_BASE.to_string(),
            api_key: Some(access_token),
            client_id,
            models: Default::default(),
            default_currency: Some("USD".to_string()),
            proxy_url,
            proxy_username,
            proxy_password,
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

/// Login to GitHub Copilot (stub when provider-copilot feature is disabled)
#[cfg(not(feature = "provider-copilot"))]
pub async fn cmd_auth_copilot(_pat: Option<String>, _client_id: Option<String>) -> Result<()> {
    anyhow::bail!("Copilot support is not compiled in. Rebuild with --features provider-copilot");
}
