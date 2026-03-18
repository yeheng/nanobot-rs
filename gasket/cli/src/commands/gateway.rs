//! Gateway 命令实现

use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::info;

use gasket_core::agent::memory::MemoryStore;
use gasket_core::agent::{AgentLoop, SubagentManager};
#[allow(unused_imports)]
use gasket_core::channels::Channel;
use gasket_core::channels::OutboundSenderRegistry;
use gasket_core::config::{load_config, ModelRegistry};
use gasket_core::cron::CronService;
use gasket_core::providers::ProviderRegistry;
use gasket_core::token_tracker::ModelPricing;
use gasket_core::tools::CronTool;
use gasket_core::tools::{MessageTool, ToolMetadata};

/// Run the gateway command
pub async fn cmd_gateway() -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;

    // Validate configuration before starting
    if let Err(errors) = config.validate() {
        println!("{}", "Configuration validation failed:".red());
        for error in &errors {
            println!("  - {}", error);
        }
        println!("\nPlease fix the configuration and try again.");
        return Ok(());
    }

    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".gasket");

    // Check if any channels are configured
    let has_telegram = config.channels.telegram.as_ref().is_some_and(|c| c.enabled);
    let has_discord = config.channels.discord.as_ref().is_some_and(|c| c.enabled);
    let has_slack = config.channels.slack.as_ref().is_some_and(|c| c.enabled);
    let has_feishu = config.channels.feishu.as_ref().is_some_and(|c| c.enabled);
    let has_email = config.channels.email.as_ref().is_some_and(|c| c.enabled);
    let has_dingtalk = config.channels.dingtalk.as_ref().is_some_and(|c| c.enabled);

    // WebSocket is enabled by feature flag
    let has_websocket = cfg!(feature = "all-channels");

    if !has_telegram
        && !has_discord
        && !has_slack
        && !has_feishu
        && !has_email
        && !has_dingtalk
        && !has_websocket
    {
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
    // Increased buffer size from 100 to 512 to handle burst traffic from parallel subagents
    let (bus, inbound_rx, outbound_rx) = gasket_core::bus::MessageBus::new(512);
    let bus = Arc::new(bus);

    // MemoryStore provides the underlying SqliteStore for session management
    // Create this FIRST to ensure a single connection pool is shared across all services
    let memory_store = Arc::new(MemoryStore::new().await);
    let sqlite_store = memory_store.sqlite_store().clone();

    // Create cron service with the shared SqliteStore to avoid duplicate connection pools
    let cron_service =
        Arc::new(CronService::with_store(sqlite_store.clone(), workspace.clone()).await);

    // Create agent with all dependencies
    let provider_info = crate::provider::find_provider(&config)?;
    let mut agent_config = super::registry::build_agent_config(&config);
    agent_config.model = provider_info.model.clone();

    // Handle thinking mode for gateway
    if agent_config.thinking_enabled && !provider_info.supports_thinking {
        tracing::warn!(
            "Provider '{}' does not support thinking mode. Thinking disabled.",
            provider_info.provider_name
        );
        agent_config.thinking_enabled = false;
    }

    // Build model registry and provider registry for switch_model tool
    let model_registry = Arc::new(ModelRegistry::from_config(&config.agents));
    let provider_registry = Arc::new(ProviderRegistry::from_config(&config));

    // Log available models if any are configured
    if !model_registry.is_empty() {
        info!(
            "Model switching enabled with {} model profiles: {}",
            model_registry.len(),
            model_registry.list_available_models().join(", ")
        );
    }

    let subagent_tools = Arc::new(super::registry::build_tool_registry(
        super::registry::ToolRegistryConfig {
            config: config.clone(),
            workspace: workspace.clone(),
            subagent_manager: None,
            extra_tools: vec![],
            sqlite_store: None, // Subagent doesn't need history search
            model_registry: Some(model_registry.clone()),
            provider_registry: Some(provider_registry.clone()),
        },
    ));

    let subagent_manager = Arc::new(
        SubagentManager::new(
            provider_info.provider.clone(),
            workspace.clone(),
            subagent_tools,
            bus.outbound_sender(),
        )
        .await,
    );

    #[allow(unused_mut)]
    let mut tools = super::registry::build_tool_registry(super::registry::ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_manager: Some(subagent_manager.clone()),
        extra_tools: {
            let mut ext: Vec<(Box<dyn gasket_core::tools::Tool>, ToolMetadata)> = vec![(
                Box::new(MessageTool::new(bus.outbound_sender()))
                    as Box<dyn gasket_core::tools::Tool>,
                ToolMetadata {
                    display_name: "Send Message".to_string(),
                    category: "communication".to_string(),
                    tags: vec!["message".to_string(), "send".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            )];

            ext.push((
                Box::new(CronTool::new(cron_service.clone())) as Box<dyn gasket_core::tools::Tool>,
                ToolMetadata {
                    display_name: "Schedule Task".to_string(),
                    category: "system".to_string(),
                    tags: vec!["cron".to_string(), "schedule".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            ));

            ext
        },
        sqlite_store: Some(sqlite_store),
        model_registry: Some(model_registry.clone()),
        provider_registry: Some(provider_registry.clone()),
    });

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    let agent = Arc::new(
        AgentLoop::with_memory_store_and_pricing(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools,
            memory_store,
            pricing,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?,
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Build InboundSender with auth/rate-limit middleware applied.
    let inbound_sender = gasket_core::channels::InboundSender::new(bus.inbound_sender());
    // TODO: wire up rate_limiter and auth_checker from config if needed
    #[allow(unused_variables)]
    let inbound_processor = inbound_sender.clone();

    // --- Actor Pipeline ---
    // Router Actor owns the session table (plain HashMap, zero locks).
    // Session Actors serialize per-session processing via dedicated mpsc channels.
    // Outbound Actor decouples network I/O from the agent loop.

    // 1. Start Outbound Actor (consumes outbound_rx, fire-and-forget HTTP sends)
    // Create registry from config - supports custom channels via register_custom()
    let outbound_registry = Arc::new(OutboundSenderRegistry::from_config(&config.channels));

    #[cfg(feature = "all-channels")]
    let websocket_manager = {
        let inbound_tx = bus.inbound_sender();
        Arc::new(gasket_core::channels::websocket::WebSocketManager::new(
            inbound_tx,
        ))
    };

    #[cfg(feature = "all-channels")]
    {
        let manager = websocket_manager.clone();
        tasks.push(tokio::spawn(async move {
            let app = axum::Router::new()
                .route(
                    "/ws",
                    axum::routing::get(
                        gasket_core::channels::websocket::WebSocketManager::handle_connection,
                    ),
                )
                .with_state(manager);

            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
            tracing::info!("WebSocket server listening on {}", addr);
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    if let Err(e) = axum::serve(listener, app).await {
                        tracing::error!("WebSocket server error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to bind WebSocket server port 3000: {}", e);
                }
            }
        }));
    }

    // Prepare ws_manager for router actor (only needed with all-channels)
    #[cfg(feature = "all-channels")]
    let ws_manager_for_router = Some(websocket_manager.clone());

    tasks.push(tokio::spawn(gasket_core::bus::run_outbound_actor(
        outbound_rx,
        outbound_registry,
        #[cfg(feature = "all-channels")]
        ws_manager_for_router,
    )));

    // 2. Start Router Actor (dispatches inbound to per-session channels)
    {
        let outbound_tx = bus.outbound_sender();
        let agent_for_router = agent.clone();
        let manager_for_router = Some(subagent_manager.clone());
        tasks.push(tokio::spawn(gasket_core::bus::run_router_actor(
            inbound_rx,
            outbound_tx,
            agent_for_router,
            manager_for_router,
        )));
    }

    // --- Heartbeat service ---
    {
        let heartbeat = gasket_core::heartbeat::HeartbeatService::new(workspace.clone());
        let bus_for_heartbeat = bus.clone();
        tasks.push(tokio::spawn(async move {
            heartbeat
                .run(|task_text| {
                    let bus_inner = bus_for_heartbeat.clone();
                    async move {
                        let inbound = gasket_core::bus::events::InboundMessage {
                            channel: gasket_core::bus::ChannelType::Cli,
                            sender_id: "heartbeat".to_string(),
                            chat_id: "heartbeat".to_string(),
                            content: task_text,
                            media: None,
                            metadata: None,
                            timestamp: chrono::Utc::now(),
                            trace_id: None,
                        };
                        bus_inner.publish_inbound(inbound).await;
                    }
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
                match cron_svc.get_due_jobs().await {
                    Ok(due) => {
                        for job in due {
                            tracing::info!("Cron job due: {} ({})", job.name, job.id);
                            let channel = job
                                .channel
                                .as_deref()
                                .and_then(|c| serde_json::from_value(serde_json::json!(c)).ok())
                                .unwrap_or(gasket_core::bus::ChannelType::Cli);
                            let chat_id = job.chat_id.clone().unwrap_or_else(|| "cron".to_string());
                            let inbound = gasket_core::bus::events::InboundMessage {
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
                    Err(e) => {
                        tracing::error!("Failed to get due cron jobs: {}", e);
                    }
                }
            }
        }));
    }

    // --- Start all configured channels using unified initializer ---
    let channel_errors = start_channels(&config, &inbound_processor, &mut tasks);
    if !channel_errors.is_empty() {
        println!(
            "{}",
            "Warning: Some channels failed to initialize:".yellow()
        );
        for error in &channel_errors {
            println!("  - {}", error);
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

/// Unified channel initializer
///
/// This function encapsulates the pattern of:
/// 1. Checking if a channel is enabled in config
/// 2. Validating channel configuration
/// 3. Creating the channel instance
/// 4. Spawning a task to run it
/// 5. Adding the task to the tasks list
///
/// Returns a list of initialization errors for channels that failed to start.
#[allow(unused_variables, clippy::ptr_arg)]
fn start_channels(
    config: &gasket_core::Config,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Vec<String> {
    #[allow(unused_mut)]
    let mut errors = Vec::new();

    // Start Telegram if configured
    #[cfg(feature = "telegram")]
    if let Some(telegram_config) = &config.channels.telegram {
        if telegram_config.enabled {
            if let Err(e) = start_telegram_channel(telegram_config, inbound_processor, tasks) {
                errors.push(format!("Telegram: {}", e));
            }
        }
    }

    // Start Discord if configured
    #[cfg(feature = "discord")]
    if let Some(discord_config) = &config.channels.discord {
        if discord_config.enabled {
            if let Err(e) = start_discord_channel(discord_config, inbound_processor, tasks) {
                errors.push(format!("Discord: {}", e));
            }
        }
    }

    // Start Slack if configured
    #[cfg(feature = "slack")]
    if let Some(slack_config) = &config.channels.slack {
        if slack_config.enabled {
            if let Err(e) = start_slack_channel(slack_config, inbound_processor, tasks) {
                errors.push(format!("Slack: {}", e));
            }
        }
    }

    // Start Feishu if configured
    #[cfg(feature = "feishu")]
    if let Some(feishu_config) = &config.channels.feishu {
        if feishu_config.enabled {
            if let Err(e) = start_feishu_channel(feishu_config, inbound_processor, tasks) {
                errors.push(format!("Feishu: {}", e));
            }
        }
    }

    // Start Email if configured
    #[cfg(feature = "email")]
    if let Some(email_config) = &config.channels.email {
        if email_config.enabled {
            // Validate email configuration first
            if !email_config.has_valid_config() {
                errors.push(
                    "Email: incomplete configuration (requires IMAP or SMTP with from_address)"
                        .to_string(),
                );
            } else if let Err(e) = start_email_channel(email_config, inbound_processor, tasks) {
                errors.push(format!("Email: {}", e));
            }
        }
    }

    // Start DingTalk if configured
    #[cfg(feature = "dingtalk")]
    if let Some(dingtalk_config) = &config.channels.dingtalk {
        if dingtalk_config.enabled {
            if let Err(e) = start_dingtalk_channel(dingtalk_config, inbound_processor, tasks) {
                errors.push(format!("DingTalk: {}", e));
            }
        }
    }

    if !errors.is_empty() {
        tracing::warn!("{} channel(s) failed to initialize", errors.len());
    }

    errors
}

/// Start a single Telegram channel
#[cfg(feature = "telegram")]
fn start_telegram_channel(
    telegram_config: &gasket_core::config::TelegramConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Telegram channel", "✓".green());

    let telegram_cfg = gasket_core::channels::telegram::TelegramConfig {
        token: telegram_config.token.clone(),
        allow_from: telegram_config.allow_from.clone(),
    };

    let telegram_channel = gasket_core::channels::telegram::TelegramChannel::new(
        telegram_cfg,
        inbound_processor.raw_sender(),
    );

    tasks.push(tokio::spawn(async move {
        if let Err(e) = telegram_channel.start().await {
            tracing::error!("Telegram channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Discord channel
#[cfg(feature = "discord")]
fn start_discord_channel(
    discord_config: &gasket_core::config::DiscordConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Discord channel", "✓".green());

    let discord_cfg = gasket_core::channels::discord::DiscordConfig {
        token: discord_config.token.clone(),
        allow_from: discord_config.allow_from.clone(),
    };

    let discord_channel = gasket_core::channels::discord::DiscordChannel::new(
        discord_cfg,
        inbound_processor.raw_sender(),
    );

    tasks.push(tokio::spawn(async move {
        if let Err(e) = discord_channel.start_bot().await {
            tracing::error!("Discord channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Slack channel
#[cfg(feature = "slack")]
fn start_slack_channel(
    slack_config: &gasket_core::config::SlackConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Slack channel", "✓".green());

    let slack_cfg = gasket_core::channels::slack::SlackConfig {
        bot_token: slack_config.bot_token.clone(),
        app_token: slack_config.app_token.clone(),
        group_policy: slack_config.group_policy.clone(),
        allow_from: slack_config.allow_from.clone(),
    };

    let slack_channel =
        gasket_core::channels::slack::SlackChannel::new(slack_cfg, inbound_processor.raw_sender());

    tasks.push(tokio::spawn(async move {
        if let Err(e) = slack_channel.start_bot().await {
            tracing::error!("Slack channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Feishu channel
#[cfg(feature = "feishu")]
fn start_feishu_channel(
    feishu_config: &gasket_core::config::FeishuConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Feishu channel", "✓".green());

    let feishu_cfg = gasket_core::channels::feishu::FeishuConfig {
        app_id: feishu_config.app_id.clone(),
        app_secret: feishu_config.app_secret.clone(),
        verification_token: feishu_config.verification_token.clone(),
        encrypt_key: feishu_config.encrypt_key.clone(),
        allow_from: feishu_config.allow_from.clone(),
    };

    let mut feishu_channel =
        gasket_core::channels::feishu::FeishuChannel::new(feishu_cfg, inbound_processor.clone());

    tasks.push(tokio::spawn(async move {
        if let Err(e) = feishu_channel.start().await {
            tracing::error!("Feishu channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Email channel
#[cfg(feature = "email")]
fn start_email_channel(
    email_config: &gasket_core::config::EmailConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Email channel", "✓".green());

    let email_cfg = gasket_core::channels::email::EmailConfig {
        imap_host: email_config.imap_host.clone().unwrap_or_default(),
        imap_port: email_config.imap_port,
        imap_username: email_config.imap_username.clone().unwrap_or_default(),
        imap_password: email_config.imap_password.clone().unwrap_or_default(),
        smtp_host: email_config.smtp_host.clone().unwrap_or_default(),
        smtp_port: email_config.smtp_port,
        smtp_username: email_config.smtp_username.clone().unwrap_or_default(),
        smtp_password: email_config.smtp_password.clone().unwrap_or_default(),
        from_address: email_config.from_address.clone().unwrap_or_default(),
        allow_from: email_config.allow_from.clone(),
        consent_granted: email_config.consent_granted,
    };

    let email_channel =
        gasket_core::channels::email::EmailChannel::new(email_cfg, inbound_processor.raw_sender());

    tasks.push(tokio::spawn(async move {
        if let Err(e) = email_channel.start_polling().await {
            tracing::error!("Email channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single DingTalk channel
#[cfg(feature = "dingtalk")]
fn start_dingtalk_channel(
    dingtalk_config: &gasket_core::config::DingTalkConfig,
    inbound_processor: &gasket_core::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} DingTalk channel", "✓".green());

    let dingtalk_cfg = gasket_core::channels::dingtalk::DingTalkConfig {
        webhook_url: dingtalk_config.webhook_url.clone(),
        secret: dingtalk_config.secret.clone(),
        access_token: dingtalk_config.access_token.clone(),
        allow_from: dingtalk_config.allow_from.clone(),
    };

    let mut dingtalk_channel = gasket_core::channels::dingtalk::DingTalkChannel::new(
        dingtalk_cfg,
        inbound_processor.clone(),
    );

    tasks.push(tokio::spawn(async move {
        if let Err(e) = dingtalk_channel.start().await {
            tracing::error!("DingTalk channel error: {}", e);
        }
    }));

    Ok(())
}
