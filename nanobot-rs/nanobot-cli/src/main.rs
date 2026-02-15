//! nanobot CLI

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;

use nanobot_core::agent::{AgentConfig, AgentLoop};
use nanobot_core::config::{load_config, Config, ConfigLoader};
use nanobot_core::providers::OpenAIProvider;

/// 🐈 nanobot - A lightweight AI assistant
#[derive(Parser)]
#[command(name = "nanobot")]
#[command(version = "2.0.0")]
#[command(about = "A lightweight personal AI assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration
    Onboard,

    /// Show status
    Status,

    /// Chat with the agent
    Agent {
        /// Message to send (if not provided, enters interactive mode)
        #[arg(short, long)]
        message: Option<String>,

        /// Show logs during chat
        #[arg(long)]
        logs: bool,

        /// Disable Markdown rendering
        #[arg(long)]
        no_markdown: bool,
    },

    /// Start the gateway (for chat channels)
    Gateway,

    /// Manage chat channels
    Channels {
        #[command(subcommand)]
        command: ChannelsCommands,
    },
}

#[derive(Subcommand)]
enum ChannelsCommands {
    /// Show status of all configured channels
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Onboard) => cmd_onboard().await,
        Some(Commands::Status) => cmd_status().await,
        Some(Commands::Agent {
            message,
            logs,
            no_markdown,
        }) => cmd_agent(message, logs, no_markdown).await,
        Some(Commands::Gateway) => cmd_gateway().await,
        Some(Commands::Channels { command }) => match command {
            ChannelsCommands::Status => cmd_channels_status().await,
        },
        None => {
            // No command - show help
            println!("🐈 nanobot v2.0.0 - A lightweight AI assistant\n");
            println!("Usage: nanobot <COMMAND>\n");
            println!("Commands:");
            println!("  onboard   Initialize configuration");
            println!("  status    Show status");
            println!("  agent     Chat with the agent");
            println!("  channels  Manage chat channels");
            println!("  gateway   Start the gateway\n");
            println!("Run 'nanobot --help' for more information.");
            Ok(())
        }
    }
}

async fn cmd_onboard() -> Result<()> {
    println!("🐈 Initializing nanobot...\n");

    let loader = ConfigLoader::new();
    let config_path = loader.config_path();

    if loader.exists() {
        println!("Configuration already exists at: {:?}", config_path);
        println!("Edit it manually to add your API keys.");
    } else {
        // Create default config
        let _config = loader.init_default()?;
        println!("Created configuration at: {:?}", config_path);
        println!("\nEdit the config to add your API key:");
        println!("  providers:");
        println!("    openrouter:");
        println!("      apiKey: sk-or-v1-xxx");
    }

    Ok(())
}

async fn cmd_status() -> Result<()> {
    let config = load_config().context("Failed to load config")?;

    println!("🐈 nanobot status\n");
    println!("Configuration: {:?}", ConfigLoader::new().config_path());

    if config.providers.is_empty() {
        println!("\n{}", "⚠️  No providers configured".yellow());
        println!("Run 'nanobot onboard' to get started.");
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

async fn cmd_agent(message: Option<String>, logs: bool, no_markdown: bool) -> Result<()> {
    // Enable debug logging if requested
    if logs {
        tracing_subscriber::fmt()
            .with_env_filter(Level::DEBUG.to_string())
            .try_init()
            .ok();
    }

    let config = load_config().context("Failed to load config")?;
    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".nanobot");

    // Find a provider
    let (provider, model) = find_provider(&config)?;

    // Create agent
    let agent_config = AgentConfig {
        model,
        max_iterations: config.agents.defaults.max_iterations,
        temperature: config.agents.defaults.temperature,
        max_tokens: config.agents.defaults.max_tokens,
        memory_window: config.agents.defaults.memory_window,
        restrict_to_workspace: config.tools.restrict_to_workspace,
    };

    let agent = AgentLoop::new(Arc::new(provider), workspace, agent_config);
    let render_md = !no_markdown;

    match message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let response = agent.process_direct(&msg, "cli:direct").await?;
            print_response(&response, render_md);
        }
        None => {
            // Interactive mode
            println!("🐈 nanobot interactive mode. Type '/help' for commands, '/exit' to quit.\n");

            let mut line_editor = Reedline::create();
            let prompt =
                DefaultPrompt::new(DefaultPromptSegment::Empty, DefaultPromptSegment::Empty);

            loop {
                match line_editor.read_line(&prompt) {
                    Ok(Signal::Success(line)) => {
                        let line = line.trim();

                        if line.is_empty() {
                            continue;
                        }

                        // Check for exit commands
                        if matches!(line, "exit" | "quit" | "/exit" | "/quit" | ":q") {
                            println!("Goodbye! 🐈");
                            break;
                        }

                        // Process the message
                        match agent.process_direct(line, "cli:interactive").await {
                            Ok(response) => {
                                println!();
                                print_response(&response, render_md);
                                println!();
                            }
                            Err(e) => println!("\n{} {}\n", "Error:".red(), e),
                        }
                    }
                    Ok(Signal::CtrlC) | Ok(Signal::CtrlD) => {
                        println!("\nGoodbye! 🐈");
                        break;
                    }
                    Err(e) => {
                        println!("Error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Print response with optional Markdown rendering
fn print_response(response: &str, render_md: bool) {
    #[cfg(feature = "markdown")]
    if render_md {
        use termimad::MadSkin;
        let skin = MadSkin::default();
        skin.print_text(response);
        return;
    }

    // Fallback to plain text
    println!("{}", response);
}

async fn cmd_gateway() -> Result<()> {
    let config = load_config().context("Failed to load config")?;
    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".nanobot");

    // Check if any channels are configured
    let has_telegram = config.channels.telegram.as_ref().is_some_and(|c| c.enabled);
    let has_discord = config.channels.discord.as_ref().is_some_and(|c| c.enabled);
    let has_slack = config.channels.slack.as_ref().is_some_and(|c| c.enabled);

    if !has_telegram && !has_discord && !has_slack {
        println!("{}", "⚠️  No channels configured".yellow());
        println!("Add a channel to your config:");
        println!("\n  channels:");
        println!("    telegram:");
        println!("      enabled: true");
        println!("      token: \"YOUR_BOT_TOKEN\"");
        println!("      allow_from: []");
        return Ok(());
    }

    println!("🐈 Starting gateway...\n");

    // Create message bus
    #[allow(unused_variables)]
    let bus = nanobot_core::bus::MessageBus::new(100);

    // Create agent
    let (provider, model) = find_provider(&config)?;
    let agent_config = AgentConfig {
        model,
        max_iterations: config.agents.defaults.max_iterations,
        temperature: config.agents.defaults.temperature,
        max_tokens: config.agents.defaults.max_tokens,
        memory_window: config.agents.defaults.memory_window,
        restrict_to_workspace: config.tools.restrict_to_workspace,
    };

    #[allow(unused_variables)]
    let agent = Arc::new(AgentLoop::new(Arc::new(provider), workspace, agent_config));

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Start Telegram if configured
    #[cfg(feature = "telegram")]
    if let Some(telegram_config) = &config.channels.telegram {
        if telegram_config.enabled {
            println!("{} Telegram channel", "✓".green());

            let telegram_cfg = nanobot_core::channels::telegram::TelegramConfig {
                token: telegram_config.token.clone(),
                allow_from: telegram_config.allow_from.clone(),
            };

            let telegram_channel =
                nanobot_core::channels::telegram::TelegramChannel::new(telegram_cfg, bus.clone());

            let agent_clone = agent.clone();
            let bus_clone = bus.clone();

            let task = tokio::spawn(async move {
                // Start a task to process inbound messages
                let agent_for_handler = agent_clone.clone();
                let bus_for_handler = bus_clone.clone();

                tokio::spawn(async move {
                    loop {
                        if let Some(msg) = bus_for_handler.consume_inbound().await {
                            match agent_for_handler
                                .process_direct(&msg.content, &msg.session_key())
                                .await
                            {
                                Ok(response) => {
                                    let outbound = nanobot_core::bus::events::OutboundMessage {
                                        channel: msg.channel,
                                        chat_id: msg.chat_id,
                                        content: response,
                                        metadata: None,
                                    };
                                    bus_for_handler.publish_outbound(outbound).await;
                                }
                                Err(e) => {
                                    tracing::error!("Error processing message: {}", e);
                                }
                            }
                        }
                    }
                });

                // This will block
                let _ = telegram_channel.start().await;
            });

            tasks.push(task);
        }
    }

    // Start Discord if configured
    #[cfg(feature = "discord")]
    if let Some(discord_config) = &config.channels.discord {
        if discord_config.enabled {
            println!("{} Discord channel", "✓".green());

            let discord_cfg = nanobot_core::channels::discord::DiscordConfig {
                token: discord_config.token.clone(),
                allow_from: discord_config.allow_from.clone(),
            };

            let discord_channel =
                nanobot_core::channels::discord::DiscordChannel::new(discord_cfg, bus.clone());

            let task = tokio::spawn(async move {
                let _ = discord_channel.start().await;
            });

            tasks.push(task);
        }
    }

    println!("\n🐈 Gateway running. Press Ctrl+C to stop.\n");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c().await?;
    println!("\n🐈 Shutting down gracefully...");

    // Abort all tasks
    for task in tasks {
        task.abort();
    }

    Ok(())
}

/// Find a configured provider
fn find_provider(config: &Config) -> Result<(OpenAIProvider, String)> {
    // Try providers in order of preference
    let provider_order = ["openrouter", "openai", "anthropic"];

    for name in &provider_order {
        if let Some(provider_config) = config.providers.get(*name) {
            if let Some(api_key) = &provider_config.api_key {
                let model = config
                    .agents
                    .defaults
                    .model
                    .clone()
                    .unwrap_or_else(|| "gpt-4o".to_string());

                let provider = match *name {
                    "openrouter" => OpenAIProvider::openrouter(api_key),
                    "anthropic" => OpenAIProvider::anthropic(api_key),
                    _ => OpenAIProvider::new(
                        api_key,
                        provider_config.api_base.clone(),
                        Some(model.clone()),
                    ),
                };

                return Ok((provider, model));
            }
        }
    }

    anyhow::bail!(
        "No API key configured. Run 'nanobot onboard' and add your API key to ~/.nanobot/config.json"
    )
}

/// Show status of all configured channels
async fn cmd_channels_status() -> Result<()> {
    println!("{}\n", "Channel Status".bold());

    let config = load_config().context("Failed to load configuration")?;

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
    let check_credential = |key: &Option<String>| match key {
        Some(k) if !k.is_empty() => {
            if k.starts_with("${") {
                has_env_credential(k)
            } else {
                "✓"
            }
        }
        _ => "✗",
    };

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

    // Check Email
    #[cfg(feature = "email")]
    {
        if let Some(email) = &config.channels.email {
            has_channels = true;
            let status = if email.enabled { "enabled" } else { "disabled" };

            println!("{}", "Email".blue().bold());
            println!("  Status:     {}", status);
            println!(
                "  IMAP:       {}",
                if email.imap_host.is_some() {
                    "✓"
                } else {
                    "✗"
                }
            );
            println!(
                "  SMTP:       {}",
                if email.smtp_host.is_some() {
                    "✓"
                } else {
                    "✗"
                }
            );
            println!();
        }
    }

    if !has_channels {
        println!("No channels configured.");
        println!("\nAdd channel configuration to ~/.nanobot/config.json");
        println!("Example:");
        println!(
            r#"
{{
  "channels": {{
    "telegram": {{
      "enabled": true,
      "token": "YOUR_BOT_TOKEN",
      "allowFrom": []
    }}
  }}
}}
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
        "  Email:    {}",
        if cfg!(feature = "email") {
            "✓"
        } else {
            "✗"
        }
    );

    Ok(())
}
