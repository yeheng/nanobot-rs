//! nanobot CLI

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use nanobot_core::channels::manager::ChannelManager;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

use nanobot_core::agent::{AgentConfig, AgentLoop, AgentResponse};
use nanobot_core::config::{load_config, Config, ConfigLoader, ProviderConfig};
use nanobot_core::providers::{LlmProvider, ModelSpec, OpenAICompatibleProvider};
use nanobot_core::tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MessageTool, ReadFileTool, SpawnTool,
    ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};

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

        /// Enable thinking/reasoning mode for deep reasoning models
        #[arg(long)]
        thinking: bool,
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
    // Initialize logging and OpenTelemetry
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    // Try to initialize OpenTelemetry, fall back to plain logging if unavailable
    if !init_telemetry(env_filter.clone()) {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_level(true)
            .with_ansi(true)
            .init();
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Onboard) => cmd_onboard().await,
        Some(Commands::Status) => cmd_status().await,
        Some(Commands::Agent {
            message,
            logs,
            no_markdown,
            thinking,
        }) => cmd_agent(message, logs, no_markdown, thinking).await,
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
    let workspace = nanobot_core::config::config_dir();

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
            include_str!("../../nanobot-core/workspace/AGENTS.md"),
        ),
        (
            "SOUL.md",
            include_str!("../../nanobot-core/workspace/SOUL.md"),
        ),
        (
            "USER.md",
            include_str!("../../nanobot-core/workspace/USER.md"),
        ),
        (
            "TOOLS.md",
            include_str!("../../nanobot-core/workspace/TOOLS.md"),
        ),
        (
            "HEARTBEAT.md",
            include_str!("../../nanobot-core/workspace/HEARTBEAT.md"),
        ),
        (
            "memory/MEMORY.md",
            include_str!("../../nanobot-core/workspace/memory/MEMORY.md"),
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

/// Build AgentConfig from the config file, applying defaults for zero-valued fields.
fn build_agent_config(config: &Config) -> AgentConfig {
    let defaults = AgentConfig::default();
    AgentConfig {
        model: String::new(), // caller overrides with resolved model
        max_iterations: match config.agents.defaults.max_iterations {
            0 => defaults.max_iterations,
            v => v,
        },
        temperature: config.agents.defaults.temperature,
        max_tokens: match config.agents.defaults.max_tokens {
            0 => defaults.max_tokens,
            v => v,
        },
        memory_window: match config.agents.defaults.memory_window {
            0 => defaults.memory_window,
            v => v,
        },
        max_tool_result_chars: defaults.max_tool_result_chars,
        thinking_enabled: config.agents.defaults.thinking_enabled,
    }
}

async fn cmd_agent(
    message: Option<String>,
    logs: bool,
    no_markdown: bool,
    thinking: bool,
) -> Result<()> {
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
    let provider_info = find_provider(&config)?;

    // Create agent config
    let mut agent_config = build_agent_config(&config);
    agent_config.model = provider_info.model;

    // Handle thinking mode
    if thinking || agent_config.thinking_enabled {
        if provider_info.supports_thinking {
            agent_config.thinking_enabled = true;
        } else {
            // Warn if thinking is requested but not supported
            println!(
                "{} Provider '{}' does not support thinking mode. Thinking disabled.",
                "⚠️".yellow(),
                provider_info.provider_name
            );
            agent_config.thinking_enabled = false;
        }
    }

    // Build tool registry (CLI mode: no bus/cron, but support web tools)
    let restrict = config.tools.restrict_to_workspace;
    let allowed_dir = if restrict {
        Some(workspace.clone())
    } else {
        None
    };

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ReadFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(WriteFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(EditFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(ListDirTool::new(allowed_dir)));
    tools.register(Box::new(ExecTool::new(
        workspace.clone(),
        std::time::Duration::from_secs(120),
        restrict,
    )));
    tools.register(Box::new(WebFetchTool::new()));
    tools.register(Box::new(WebSearchTool::new(Some(config.tools.web.clone()))));
    tools.register(Box::new(SpawnTool::new()));

    let agent = AgentLoop::new(provider_info.provider, workspace, agent_config, tools)
        .context("Failed to initialize agent (check workspace bootstrap files)")?;
    let render_md = !no_markdown;

    match message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let response = agent.process_direct(&msg, "cli:direct").await?;
            print_response_with_reasoning(&response, render_md);
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
                                print_response_with_reasoning(&response, render_md);
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

/// Print reasoning content in a styled block
fn print_reasoning_block(reasoning: &str) {
    // Print a header with dimmed color and box drawing
    println!(
        "{}",
        "┌─ Thinking ─────────────────────────────────".dimmed()
    );

    // Print reasoning content with dimmed/italic style
    // Split by lines to handle multi-line reasoning
    for line in reasoning.lines() {
        println!("│ {}", line.dimmed().italic());
    }

    // Print footer
    println!(
        "{}",
        "└─────────────────────────────────────────────".dimmed()
    );
}

/// Print response with optional reasoning content and Markdown rendering
fn print_response_with_reasoning(response: &AgentResponse, render_md: bool) {
    // Print reasoning content first (if present) with special styling
    if let Some(ref reasoning) = response.reasoning_content {
        if !reasoning.is_empty() {
            print_reasoning_block(reasoning);
            println!(); // Add blank line between reasoning and main response
        }
    }

    // Print main response content
    print_response(&response.content, render_md);
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
    let has_feishu = config.channels.feishu.as_ref().is_some_and(|c| c.enabled);

    if !has_telegram && !has_discord && !has_slack && !has_feishu {
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

    // Create message bus — receivers are split out at creation time, no Mutex needed
    let (bus, mut inbound_rx, outbound_rx) = nanobot_core::bus::MessageBus::new(100);
    let bus = Arc::new(bus);

    // Create cron service
    let cron_service = Arc::new(nanobot_core::cron::CronService::new(workspace.clone()));

    // Create agent with all dependencies
    let provider_info = find_provider(&config)?;
    let mut agent_config = build_agent_config(&config);
    agent_config.model = provider_info.model;

    // Handle thinking mode for gateway
    if agent_config.thinking_enabled && !provider_info.supports_thinking {
        tracing::warn!(
            "Provider '{}' does not support thinking mode. Thinking disabled.",
            provider_info.provider_name
        );
        agent_config.thinking_enabled = false;
    }

    // Start MCP servers (if configured)
    let mcp_tools = if !config.tools.mcp_servers.is_empty() {
        println!("Starting MCP servers...");
        let (_mcp_manager, tools) =
            nanobot_core::mcp::start_mcp_servers(&config.tools.mcp_servers).await;
        println!("  {} MCP tools loaded", tools.len());
        tools
    } else {
        Vec::new()
    };

    // Build tool registry externally
    let restrict = config.tools.restrict_to_workspace;
    let allowed_dir = if restrict {
        Some(workspace.clone())
    } else {
        None
    };

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ReadFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(WriteFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(EditFileTool::new(allowed_dir.clone())));
    tools.register(Box::new(ListDirTool::new(allowed_dir)));
    tools.register(Box::new(ExecTool::new(
        workspace.clone(),
        std::time::Duration::from_secs(120),
        restrict,
    )));
    tools.register(Box::new(WebFetchTool::new()));
    tools.register(Box::new(WebSearchTool::new(Some(config.tools.web.clone()))));
    tools.register(Box::new(SpawnTool::new()));
    tools.register(Box::new(MessageTool::new(bus.clone())));
    tools.register(Box::new(CronTool::new(cron_service.clone())));
    for mcp_tool in mcp_tools {
        tools.register(mcp_tool);
    }

    let agent = Arc::new(
        AgentLoop::new(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools,
        )
        .context("Failed to initialize agent (check workspace bootstrap files)")?,
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // --- Channel manager + outbound router ---
    let channel_manager = Arc::new(ChannelManager::new(bus.clone()));
    tasks.push(channel_manager.spawn_outbound_router(outbound_rx));

    // --- Inbound message handler ---
    {
        let agent_for_handler = agent.clone();
        let bus_for_handler = bus.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(msg) = inbound_rx.recv().await {
                let agent_clone = agent_for_handler.clone();
                let bus_clone = bus_for_handler.clone();
                // Process each message concurrently
                tokio::spawn(async move {
                    match agent_clone
                        .process_direct(&msg.content, &msg.session_key())
                        .await
                    {
                        Ok(response) => {
                            let outbound = nanobot_core::bus::events::OutboundMessage {
                                channel: msg.channel,
                                chat_id: msg.chat_id,
                                content: response.content,
                                metadata: None,
                                trace_id: None,
                            };
                            bus_clone.publish_outbound(outbound).await;
                        }
                        Err(e) => {
                            tracing::error!("Error processing message: {}", e);
                        }
                    }
                });
            }
        }));
    }

    // --- Heartbeat service ---
    {
        let heartbeat = nanobot_core::heartbeat::HeartbeatService::new(workspace.clone());
        let bus_for_heartbeat = bus.clone();
        tasks.push(tokio::spawn(async move {
            heartbeat
                .run(|task_text| {
                    let bus_inner = bus_for_heartbeat.clone();
                    tokio::spawn(async move {
                        let inbound = nanobot_core::bus::events::InboundMessage {
                            channel: nanobot_core::bus::cli(),
                            sender_id: "heartbeat".to_string(),
                            chat_id: "heartbeat".to_string(),
                            content: task_text,
                            media: None,
                            metadata: None,
                            timestamp: chrono::Utc::now(),
                            trace_id: None,
                        };
                        bus_inner.publish_inbound(inbound).await;
                    });
                })
                .await;
        }));
    }

    // --- Cron checking loop ---
    {
        let cron_svc = cron_service.clone();
        let bus_for_cron = bus.clone();
        tasks.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let due = cron_svc.get_due_jobs().await;
                for job in due {
                    tracing::info!("Cron job due: {} ({})", job.name, job.id);
                    let channel = job
                        .channel
                        .as_deref()
                        .and_then(|c| serde_json::from_value(serde_json::json!(c)).ok())
                        .unwrap_or_else(nanobot_core::bus::cli);
                    let chat_id = job.chat_id.clone().unwrap_or_else(|| "cron".to_string());
                    let inbound = nanobot_core::bus::events::InboundMessage {
                        channel,
                        sender_id: "cron".to_string(),
                        chat_id,
                        content: job.message.clone(),
                        media: None,
                        metadata: None,
                        timestamp: chrono::Utc::now(),
                        trace_id: None,
                    };
                    bus_for_cron.publish_inbound(inbound).await;
                    cron_svc.mark_job_run(&job.id).await;
                }
            }
        }));
    }

    // Start Telegram if configured
    #[cfg(feature = "telegram")]
    if let Some(telegram_config) = &config.channels.telegram {
        if telegram_config.enabled {
            println!("{} Telegram channel", "✓".green());

            let telegram_cfg = nanobot_core::channels::telegram::TelegramConfig {
                token: telegram_config.token.clone(),
                allow_from: telegram_config.allow_from.clone(),
            };

            let telegram_channel = nanobot_core::channels::telegram::TelegramChannel::new(
                telegram_cfg,
                inbound_processor.clone(),
            );

            let task = tokio::spawn(async move {
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

            let discord_channel = nanobot_core::channels::discord::DiscordChannel::new(
                discord_cfg,
                inbound_processor.clone(),
            );

            let task = tokio::spawn(async move {
                let _ = discord_channel.start_bot().await;
            });

            tasks.push(task);
        }
    }

    // Start Feishu if configured
    #[cfg(feature = "feishu")]
    if let Some(feishu_config) = &config.channels.feishu {
        if feishu_config.enabled {
            println!("{} Feishu channel", "✓".green());

            let feishu_cfg = nanobot_core::channels::feishu::FeishuConfig {
                app_id: feishu_config.app_id.clone(),
                app_secret: feishu_config.app_secret.clone(),
                verification_token: feishu_config.verification_token.clone(),
                encrypt_key: feishu_config.encrypt_key.clone(),
                allow_from: feishu_config.allow_from.clone(),
            };

            let mut feishu_channel = nanobot_core::channels::feishu::FeishuChannel::new(
                feishu_cfg,
                inbound_processor.clone(),
            );

            let task = tokio::spawn(async move {
                let _ = feishu_channel.start().await;
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

/// Provider information returned by find_provider
struct ProviderInfo {
    /// The provider instance
    provider: Arc<dyn LlmProvider>,
    /// The model name to use
    model: String,
    /// Provider name (e.g., "zhipu", "deepseek")
    provider_name: String,
    /// Whether this provider supports thinking/reasoning mode
    supports_thinking: bool,
}

/// Build a provider instance from its name and config.
fn build_provider(
    name: &str,
    api_key: &str,
    provider_config: &nanobot_core::config::ProviderConfig,
    model: &str,
) -> Arc<dyn LlmProvider> {
    match name {
        "deepseek" => Arc::new(OpenAICompatibleProvider::deepseek(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "openrouter" => Arc::new(OpenAICompatibleProvider::openrouter(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "anthropic" => Arc::new(OpenAICompatibleProvider::anthropic(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "zhipu" => Arc::new(OpenAICompatibleProvider::zhipu(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "dashscope" => Arc::new(OpenAICompatibleProvider::dashscope(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "moonshot" => Arc::new(OpenAICompatibleProvider::moonshot(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
        "minimax" => Arc::new(OpenAICompatibleProvider::minimax(
            api_key,
            provider_config.api_base.clone(),
            model,
            None,
        )),
        "ollama" => Arc::new(OpenAICompatibleProvider::ollama(
            provider_config.api_base.clone(),
            model,
        )),
        _ => Arc::new(OpenAICompatibleProvider::openai(
            api_key,
            provider_config.api_base.clone(),
            model,
        )),
    }
}

/// Find a configured provider.
///
/// The model field supports `provider_id/model_id` format (parsed via
/// `ModelSpec`) to select a specific provider. For example:
///   - `"deepseek/deepseek-chat"` → use the deepseek provider with model deepseek-chat
///   - `"zhipu/glm-4"`           → use the zhipu provider with model glm-4
///   - `"deepseek-chat"`          → legacy behaviour, pick the first provider with an API key
fn find_provider(config: &Config) -> Result<ProviderInfo> {
    let raw_model = config
        .agents
        .defaults
        .model
        .clone()
        .unwrap_or_else(|| "gpt-4o".to_string());

    // Parse once into a strongly-typed ModelSpec
    let spec: ModelSpec = raw_model
        .parse()
        .expect("ModelSpec::from_str is infallible");

    // Helper to build ProviderInfo
    let build_info =
        |name: &str, provider_config: &ProviderConfig, api_key: &str| -> ProviderInfo {
            let provider = build_provider(name, api_key, provider_config, spec.model());
            ProviderInfo {
                provider,
                model: spec.model().to_string(),
                provider_name: name.to_string(),
                supports_thinking: provider_config.supports_thinking(name),
            }
        };

    // If the user specified a provider prefix, use that provider directly.
    if let Some(provider_name) = spec.provider() {
        let provider_config = config.providers.get(provider_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{}' specified in model '{}' is not configured in providers section",
                provider_name,
                spec
            )
        })?;

        let api_key = provider_config.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Provider '{}' has no API key configured", provider_name)
        })?;

        return Ok(build_info(provider_name, provider_config, api_key));
    }

    // First, try providers in order of preference.
    let provider_order = [
        "deepseek",
        "openrouter",
        "openai",
        "anthropic",
        "ollama",
        "zhipu",
        "dashscope",
        "moonshot",
        "minimax",
    ];

    for name in &provider_order {
        if let Some(provider_config) = config.providers.get(*name) {
            // Ollama is a local service and doesn't require an API key
            if *name == "ollama" {
                return Ok(build_info(name, provider_config, ""));
            }
            if let Some(api_key) = &provider_config.api_key {
                return Ok(build_info(name, provider_config, api_key));
            }
        }
    }

    // Fall back to any configured provider (check for ollama without api_key, or others with api_key)
    for (name, provider_config) in &config.providers {
        // Ollama is a local service and doesn't require an API key
        if name == "ollama" {
            return Ok(build_info(name, provider_config, ""));
        }
        if let Some(api_key) = &provider_config.api_key {
            return Ok(build_info(name, provider_config, api_key));
        }
    }

    anyhow::bail!(
        "No API key configured. Run 'nanobot onboard' and add your API key to ~/.nanobot/config.yaml"
    )
}

/// Show status of all configured channels
#[allow(unused_variables)]
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
            let cred = check_credential(&Some(feishu.app_id.clone()));

            println!("{}", "Feishu".magenta().bold());
            println!("  Status:     {}", status);
            println!("  App ID:     {}", cred);
            println!("  Allow From: {} users", feishu.allow_from.len());
            println!();
        }
    }

    if !has_channels {
        println!("No channels configured.");
        println!("\nAdd channel configuration to ~/.nanobot/config.yaml");
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
        "  Email:    {}",
        if cfg!(feature = "email") {
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

/// Initialize OpenTelemetry tracing (optional).
///
/// Only initializes OpenTelemetry when explicitly configured via environment
/// variables. Defaults to no exporter (logging only).
///
/// Environment variables:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint URL (e.g., http://localhost:4317)
/// - `OTEL_SDK_DISABLED=true`: Disable OpenTelemetry completely
fn init_telemetry(env_filter: EnvFilter) -> bool {
    use tracing_subscriber::util::SubscriberInitExt;

    // Check if OTEL is disabled
    if std::env::var("OTEL_SDK_DISABLED").is_ok_and(|v| v == "true") {
        return false;
    }

    // Only initialize if endpoint is explicitly configured
    let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(e) => e,
        Err(_) => return false, // No endpoint configured, skip OpenTelemetry
    };

    // Try to create OTLP exporter
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(format!("{}/v1/traces", endpoint))
        .build()
    {
        Ok(e) => e,
        Err(_) => return false,
    };

    // Create tracer provider
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer("nanobot");

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create tracing layer and initialize
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    info!("OpenTelemetry tracing enabled: {}", endpoint);
    true
}
