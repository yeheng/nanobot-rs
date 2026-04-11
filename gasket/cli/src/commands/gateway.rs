//! Gateway 命令实现

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::info;

use gasket_engine::bus_adapter::EngineHandler;
#[allow(unused_imports)]
use gasket_engine::channels::Channel;
use gasket_engine::channels::OutboundSenderRegistry;
use gasket_engine::config::{load_config, ModelRegistry};
use gasket_engine::cron::CronService;
use gasket_engine::memory::MemoryStore;
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::session::AgentSession;
use gasket_engine::token_tracker::ModelPricing;
use gasket_engine::tools::CronTool;
use gasket_engine::tools::{MessageTool, ToolMetadata};
use gasket_engine::SubagentManager;

use super::registry::CliModelResolver;
use crate::provider::setup_vault;

/// Run the gateway command
pub async fn cmd_gateway() -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;

    // Check for vault placeholders and unlock if needed (JIT setup)
    let vault = setup_vault(&config)?;

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
    let has_dingtalk = config.channels.dingtalk.as_ref().is_some_and(|c| c.enabled);

    // WebSocket is enabled by feature flag
    let has_websocket = cfg!(feature = "all-channels");

    if !has_telegram && !has_discord && !has_slack && !has_feishu && !has_dingtalk && !has_websocket
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
    let (bus, inbound_rx, outbound_rx) = gasket_engine::bus::MessageBus::new(512);
    let bus = Arc::new(bus);

    // MemoryStore provides the underlying SqliteStore for session management
    let memory_store = Arc::new(MemoryStore::new().await);

    // Create cron service with file-driven architecture (no SQLite dependency)
    // Manual refresh via 'gasket cron refresh' command
    let cron_service = Arc::new(CronService::new(workspace.clone()).await);

    // Create agent with all dependencies
    let provider_info = crate::provider::find_provider(&config, vault.as_deref())?;
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
    let mut provider_registry = ProviderRegistry::from_config(&config);
    if let Some(ref v) = vault {
        provider_registry.with_vault(v.clone());
    }
    let provider_registry = Arc::new(provider_registry);

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
        SubagentManager::with_model_resolver(
            provider_info.provider.clone(),
            workspace.clone(),
            subagent_tools,
            bus.outbound_sender(),
            Some(Arc::new(CliModelResolver {
                provider_registry: {
                    let mut r = ProviderRegistry::from_config(&config);
                    if let Some(ref v) = vault {
                        r.with_vault(v.clone());
                    }
                    r
                },
                model_registry: ModelRegistry::from_config(&config.agents),
            })),
        )
        .await,
    );

    #[allow(unused_mut)]
    let mut tools = super::registry::build_tool_registry(super::registry::ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_manager: Some(subagent_manager.clone()),
        extra_tools: {
            let mut ext: Vec<(Box<dyn gasket_engine::tools::Tool>, ToolMetadata)> = vec![(
                Box::new(MessageTool::new(bus.outbound_sender()))
                    as Box<dyn gasket_engine::tools::Tool>,
                ToolMetadata {
                    display_name: "Send Message".to_string(),
                    category: "communication".to_string(),
                    tags: vec!["message".to_string(), "send".to_string()],
                    requires_approval: false,
                    is_mutating: false,
                },
            )];

            ext.push((
                Box::new(CronTool::new(cron_service.clone()))
                    as Box<dyn gasket_engine::tools::Tool>,
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
        sqlite_store: None, // Cron service is now file-driven, no SQLite needed
        model_registry: Some(model_registry.clone()),
        provider_registry: Some(provider_registry.clone()),
    });

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    let agent = Arc::new(
        AgentSession::with_pricing(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools,
            memory_store,
            pricing,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?
        .with_spawner(subagent_manager.clone() as Arc<dyn gasket_engine::SubagentSpawner>),
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Build InboundSender with auth/rate-limit middleware applied.
    let inbound_sender = gasket_engine::channels::InboundSender::new(bus.inbound_sender());
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
        Arc::new(gasket_engine::channels::websocket::WebSocketManager::new(
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
                        gasket_engine::channels::websocket::WebSocketManager::handle_connection,
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

    tasks.push(tokio::spawn(gasket_engine::bus::run_outbound_actor(
        outbound_rx,
        outbound_registry,
        #[cfg(feature = "all-channels")]
        ws_manager_for_router,
    )));

    // 2. Start Router Actor (dispatches inbound to per-session channels)
    {
        let outbound_tx = bus.outbound_sender();
        // EngineHandler adapts AgentSession to the MessageHandler trait
        let handler = Arc::new(EngineHandler::new(agent));
        tasks.push(tokio::spawn(gasket_engine::bus::run_router_actor(
            inbound_rx,
            outbound_tx,
            handler,
        )));
    }

    // --- Background services ---
    start_heartbeat_service(&bus, &workspace, &mut tasks);
    start_cron_checker(&cron_service, &bus, &mut tasks);

    // --- Start all configured channels using unified initializer ---
    let channel_errors = start_channels(&config, vault.as_deref(), &inbound_processor, &mut tasks);
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

    // Abort all background tasks
    for task in &tasks {
        task.abort();
    }

    // Wait for tasks to finish with timeout
    use tokio::time::{timeout, Duration};
    for task in tasks {
        let _ = timeout(Duration::from_millis(500), task).await;
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
    config: &gasket_engine::Config,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Vec<String> {
    #[allow(unused_mut)]
    let mut errors = Vec::new();

    // Start Telegram if configured
    #[cfg(feature = "telegram")]
    if let Some(telegram_config) = &config.channels.telegram {
        if telegram_config.enabled {
            if let Err(e) = start_telegram_channel(telegram_config, vault, inbound_processor, tasks)
            {
                errors.push(format!("Telegram: {}", e));
            }
        }
    }

    // Start Discord if configured
    #[cfg(feature = "discord")]
    if let Some(discord_config) = &config.channels.discord {
        if discord_config.enabled {
            if let Err(e) = start_discord_channel(discord_config, vault, inbound_processor, tasks) {
                errors.push(format!("Discord: {}", e));
            }
        }
    }

    // Start Slack if configured
    #[cfg(feature = "slack")]
    if let Some(slack_config) = &config.channels.slack {
        if slack_config.enabled {
            if let Err(e) = start_slack_channel(slack_config, vault, inbound_processor, tasks) {
                errors.push(format!("Slack: {}", e));
            }
        }
    }

    // Start Feishu if configured
    #[cfg(feature = "feishu")]
    if let Some(feishu_config) = &config.channels.feishu {
        if feishu_config.enabled {
            if let Err(e) = start_feishu_channel(feishu_config, vault, inbound_processor, tasks) {
                errors.push(format!("Feishu: {}", e));
            }
        }
    }

    // Start DingTalk if configured
    #[cfg(feature = "dingtalk")]
    if let Some(dingtalk_config) = &config.channels.dingtalk {
        if dingtalk_config.enabled {
            if let Err(e) = start_dingtalk_channel(dingtalk_config, vault, inbound_processor, tasks)
            {
                errors.push(format!("DingTalk: {}", e));
            }
        }
    }

    if !errors.is_empty() {
        tracing::warn!("{} channel(s) failed to initialize", errors.len());
    }

    errors
}

/// Resolve a secret string through vault (JIT).
/// Returns the original string if no vault is available or no placeholders found.
#[allow(dead_code)]
fn resolve_channel_secret(raw: &str, vault: Option<&gasket_engine::vault::VaultStore>) -> String {
    match vault {
        Some(v) => v.resolve_text(raw).unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to resolve vault placeholder: {}. Using raw value.",
                e
            );
            raw.to_string()
        }),
        None => raw.to_string(),
    }
}

/// Start a single Telegram channel
#[cfg(feature = "telegram")]
fn start_telegram_channel(
    telegram_config: &gasket_engine::config::TelegramConfig,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Telegram channel", "✓".green());

    let resolved_token = resolve_channel_secret(&telegram_config.token, vault);
    let telegram_cfg = gasket_engine::channels::telegram::TelegramConfig {
        token: resolved_token,
        allow_from: telegram_config.allow_from.clone(),
    };

    let mut telegram_channel = gasket_engine::channels::telegram::TelegramChannel::new(
        telegram_cfg,
        inbound_processor.raw_sender(),
    );

    tasks.push(tokio::spawn(async move {
        use gasket_engine::channels::Channel;
        if let Err(e) = telegram_channel.start().await {
            tracing::error!("Telegram channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Discord channel
#[cfg(feature = "discord")]
fn start_discord_channel(
    discord_config: &gasket_engine::config::DiscordConfig,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Discord channel", "✓".green());

    let resolved_token = resolve_channel_secret(&discord_config.token, vault);
    let discord_cfg = gasket_engine::channels::discord::DiscordConfig {
        token: resolved_token,
        allow_from: discord_config.allow_from.clone(),
    };

    let mut discord_channel = gasket_engine::channels::discord::DiscordChannel::new(
        discord_cfg,
        inbound_processor.raw_sender(),
    );

    tasks.push(tokio::spawn(async move {
        use gasket_engine::channels::Channel;
        if let Err(e) = discord_channel.start().await {
            tracing::error!("Discord channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Slack channel
#[cfg(feature = "slack")]
fn start_slack_channel(
    slack_config: &gasket_engine::config::SlackConfig,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Slack channel", "✓".green());

    let resolved_bot_token = resolve_channel_secret(&slack_config.bot_token, vault);
    let resolved_app_token = resolve_channel_secret(&slack_config.app_token, vault);
    let slack_cfg = gasket_engine::channels::slack::SlackConfig {
        bot_token: resolved_bot_token,
        app_token: resolved_app_token,
        group_policy: slack_config.group_policy.clone(),
        allow_from: slack_config.allow_from.clone(),
    };

    let mut slack_channel = gasket_engine::channels::slack::SlackChannel::new(
        slack_cfg,
        inbound_processor.raw_sender(),
    );

    tasks.push(tokio::spawn(async move {
        use gasket_engine::channels::Channel;
        if let Err(e) = slack_channel.start().await {
            tracing::error!("Slack channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single Feishu channel
#[cfg(feature = "feishu")]
fn start_feishu_channel(
    feishu_config: &gasket_engine::config::FeishuConfig,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} Feishu channel", "✓".green());

    let feishu_cfg = gasket_engine::channels::feishu::FeishuConfig {
        app_id: resolve_channel_secret(&feishu_config.app_id, vault),
        app_secret: resolve_channel_secret(&feishu_config.app_secret, vault),
        verification_token: feishu_config
            .verification_token
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            .then(|| {
                resolve_channel_secret(feishu_config.verification_token.as_ref().unwrap(), vault)
            }),
        encrypt_key: feishu_config
            .encrypt_key
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            .then(|| resolve_channel_secret(feishu_config.encrypt_key.as_ref().unwrap(), vault)),
        allow_from: feishu_config.allow_from.clone(),
    };

    let mut feishu_channel =
        gasket_engine::channels::feishu::FeishuChannel::new(feishu_cfg, inbound_processor.clone());

    tasks.push(tokio::spawn(async move {
        if let Err(e) = feishu_channel.start().await {
            tracing::error!("Feishu channel error: {}", e);
        }
    }));

    Ok(())
}

/// Start a single DingTalk channel
#[cfg(feature = "dingtalk")]
fn start_dingtalk_channel(
    dingtalk_config: &gasket_engine::config::DingTalkConfig,
    vault: Option<&gasket_engine::vault::VaultStore>,
    inbound_processor: &gasket_engine::channels::InboundSender,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Result<(), String> {
    println!("{} DingTalk channel", "✓".green());

    let dingtalk_cfg = gasket_engine::channels::dingtalk::DingTalkConfig {
        webhook_url: resolve_channel_secret(&dingtalk_config.webhook_url, vault),
        secret: dingtalk_config
            .secret
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            .then(|| resolve_channel_secret(dingtalk_config.secret.as_ref().unwrap(), vault)),
        access_token: dingtalk_config
            .access_token
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            .then(|| resolve_channel_secret(dingtalk_config.access_token.as_ref().unwrap(), vault)),
        allow_from: dingtalk_config.allow_from.clone(),
    };

    let mut dingtalk_channel = gasket_engine::channels::dingtalk::DingTalkChannel::new(
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

/// Start heartbeat service that periodically sends heartbeat tasks through the bus.
fn start_heartbeat_service(
    bus: &Arc<gasket_engine::bus::MessageBus>,
    workspace: &Path,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let heartbeat = gasket_engine::heartbeat::HeartbeatService::new(workspace.to_path_buf());
    let bus_for_heartbeat = bus.clone();
    tasks.push(tokio::spawn(async move {
        heartbeat
            .run(|task_text| {
                let bus_inner = bus_for_heartbeat.clone();
                async move {
                    let inbound = gasket_engine::bus::events::InboundMessage {
                        channel: gasket_engine::bus::ChannelType::Cli,
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

/// Start cron checker that polls for due jobs every 60 seconds.
fn start_cron_checker(
    cron_service: &Arc<CronService>,
    bus: &Arc<gasket_engine::bus::MessageBus>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
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
                            .unwrap_or(gasket_engine::bus::ChannelType::Cli);
                        let chat_id = job.chat_id.clone().unwrap_or_else(|| "cron".to_string());
                        let inbound = gasket_engine::bus::events::InboundMessage {
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
                        // Update next_run time in memory (no persistence needed)
                        let next_run = {
                            let mut job = job.clone();
                            job.update_next_run();
                            job.next_run
                        };
                        cron_svc.update_job_next_run(&job.id, next_run).await;
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get due cron jobs: {}", e);
                }
            }
        }
    }));
}
