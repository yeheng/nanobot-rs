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
pub async fn cmd_auth_copilot(pat: Option<String>, _client_id: Option<String>) -> Result<()> {
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
    let (proxy_url, proxy_username, proxy_password) = existing_proxy.unwrap_or((None, None, None));

    if let Some(token) = pat {
        // PAT mode: validate via rig's authorize()
        println!("Validating Personal Access Token...");

        match gasket_engine::providers::CopilotProvider::validate_pat(&token).await {
            Ok(()) => {
                println!("{} Token validated successfully", "✓".green());
                save_copilot_config(
                    &loader,
                    &mut config,
                    Some(token),
                    proxy_url,
                    proxy_username,
                    proxy_password,
                )
                .await?;
            }
            Err(e) => {
                anyhow::bail!(
                    "Invalid Personal Access Token: {}\n\
                     Ensure it has 'copilot' scope.\n\
                     Create a PAT at: https://github.com/settings/tokens",
                    e
                );
            }
        }
    } else {
        // OAuth Device Flow: rig handles everything, prints to stdout
        println!("Starting GitHub OAuth Device Flow...\n");

        let gasket_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".gasket"))
            .join("gasket");

        match gasket_engine::providers::CopilotProvider::oauth_device_flow(&gasket_dir).await {
            Ok(()) => {
                println!("\n{} Successfully authenticated!", "✓".green());
                // OAuth tokens are cached by rig in gasket_dir — no api_key needed
                save_copilot_config(
                    &loader,
                    &mut config,
                    None,
                    proxy_url,
                    proxy_username,
                    proxy_password,
                )
                .await?;
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
    }

    Ok(())
}

#[cfg(feature = "provider-copilot")]
async fn save_copilot_config(
    loader: &ConfigLoader,
    config: &mut gasket_engine::config::Config,
    api_key: Option<String>,
    proxy_url: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
) -> Result<()> {
    config.providers.insert(
        "copilot".to_string(),
        gasket_engine::config::ProviderConfig {
            provider_type: gasket_engine::config::ProviderType::Openai,
            api_base: COPILOT_API_BASE.to_string(),
            api_key,
            default_model: String::new(),
            client_id: None,
            models: Default::default(),
            extra_headers: Default::default(),
            default_currency: Some("USD".to_string()),
            proxy_url,
            proxy_username,
            proxy_password,
            supports_thinking: true,
        },
    );

    loader.save(config).await?;
    println!(
        "\n{} Config saved to {:?}",
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
