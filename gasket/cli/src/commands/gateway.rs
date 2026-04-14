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
use gasket_engine::subagents::SimpleSpawner;
use gasket_engine::token_tracker::ModelPricing;
use gasket_engine::tools::CronTool;
use gasket_engine::tools::MemoryDecayTool;
use gasket_engine::tools::MemoryRefreshTool;
use gasket_engine::tools::{MessageTool, ToolMetadata, ToolRegistry};
use gasket_engine::SubagentSpawner;

use gasket_engine::broker::{MemoryBroker, SessionManager};
use gasket_engine::OutboundDispatcher;

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

    // Create message broker (replaces MessageBus)
    // P2P capacity 1024, broadcast capacity 256
    let broker: Arc<dyn gasket_engine::broker::MessageBroker> =
        Arc::new(MemoryBroker::new(1024, 256));

    // MemoryStore provides the underlying SqliteStore for session management
    let memory_store = Arc::new(MemoryStore::new().await);

    // Create MemoryManager for memory refresh operations
    let memory_manager = {
        let pool = memory_store.pool();
        let base_dir = workspace.join("memory");

        // Use NoopEmbedder for Gateway (we don't need embeddings for refresh operations)
        let embedder: Box<dyn gasket_engine::Embedder> =
            Box::new(gasket_engine::NoopEmbedder::new(384));

        Arc::new(
            gasket_engine::session::memory::MemoryManager::new(base_dir, &pool, embedder)
                .await
                .expect("Failed to initialize MemoryManager"),
        )
    };

    // Create cron service with hybrid file+database architecture
    // State (last_run/next_run) persists in SQLite, config lives in ~/.gasket/cron/*.md
    let sqlite_store = Arc::new(
        gasket_engine::memory::SqliteStore::new()
            .await
            .expect("Failed to open SQLite store for cron persistence"),
    );
    let cron_service = Arc::new(CronService::new(workspace.clone(), sqlite_store.clone()).await);

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
            subagent_spawner: None,
            extra_tools: vec![],
            sqlite_store: None, // Subagent doesn't need history search
            model_registry: Some(model_registry.clone()),
            provider_registry: Some(provider_registry.clone()),
        },
    ));

    let subagent_spawner: Arc<dyn gasket_engine::SubagentSpawner> = Arc::new(
        SimpleSpawner::new(
            provider_info.provider.clone(),
            subagent_tools,
            workspace.clone(),
        )
        .with_model_resolver(Arc::new(CliModelResolver {
            provider_registry: {
                let mut r = ProviderRegistry::from_config(&config);
                if let Some(ref v) = vault {
                    r.with_vault(v.clone());
                }
                r
            },
            model_registry: ModelRegistry::from_config(&config.agents),
        })),
    );

    let tools = Arc::new(super::registry::build_tool_registry(
        super::registry::ToolRegistryConfig {
            config: config.clone(),
            workspace: workspace.clone(),
            subagent_spawner: Some(subagent_spawner.clone()),
            extra_tools: {
                let mut ext: Vec<(Box<dyn gasket_engine::tools::Tool>, ToolMetadata)> = vec![(
                    Box::new(MessageTool::new_broker(broker.clone()))
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

                ext.push((
                    Box::new(MemoryRefreshTool::new(memory_manager.clone()))
                        as Box<dyn gasket_engine::tools::Tool>,
                    ToolMetadata {
                        display_name: "Memory Refresh".to_string(),
                        category: "system".to_string(),
                        tags: vec![
                            "memory".to_string(),
                            "refresh".to_string(),
                            "index".to_string(),
                        ],
                        requires_approval: false,
                        is_mutating: true,
                    },
                ));

                ext.push((
                    Box::new(MemoryDecayTool::new(
                        workspace.clone(),
                        sqlite_store.clone(),
                    )) as Box<dyn gasket_engine::tools::Tool>,
                    ToolMetadata {
                        display_name: "Memory Decay".to_string(),
                        category: "system".to_string(),
                        tags: vec![
                            "memory".to_string(),
                            "decay".to_string(),
                            "maintenance".to_string(),
                        ],
                        requires_approval: false,
                        is_mutating: true,
                    },
                ));

                ext
            },
            sqlite_store: None, // Cron service is now file-driven, no SQLite needed
            model_registry: Some(model_registry.clone()),
            provider_registry: Some(provider_registry.clone()),
        },
    ));

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    let agent = Arc::new(
        AgentSession::with_pricing(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools.clone(),
            memory_store,
            pricing,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?
        .with_spawner(subagent_spawner.clone()),
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Build InboundSender with broker mode (replaces bus.inbound_sender()).
    let inbound_sender =
        gasket_engine::channels::InboundSender::new_with_broker(broker.clone());
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
        Arc::new(gasket_engine::channels::websocket::WebSocketManager::new_with_broker(
            broker.clone(),
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

    // --- Broker Pipeline (replaces old Router/Session/Outbound actors) ---

    // 1. Start OutboundDispatcher (subscribes to Topic::Outbound, routes to channels)
    #[cfg(feature = "all-channels")]
    let outbound_dispatcher = OutboundDispatcher::with_websocket(
        broker.clone(),
        outbound_registry,
        websocket_manager.clone(),
    );
    #[cfg(not(feature = "all-channels"))]
    let outbound_dispatcher = OutboundDispatcher::new(broker.clone(), outbound_registry);
    tasks.push(tokio::spawn(outbound_dispatcher.run()));

    // 2. Start SessionManager (subscribes to Topic::Inbound, dispatches to per-session tasks)
    {
        let handler = Arc::new(EngineHandler::new(agent));
        let session_mgr =
            SessionManager::new(broker.clone(), handler, std::time::Duration::from_secs(3600));
        tasks.push(tokio::spawn(session_mgr.run()));
    }

    // --- Background services ---
    start_heartbeat_service(&broker, &workspace, &mut tasks);
    start_cron_checker(
        &cron_service,
        &broker,
        tools.clone(),
        subagent_spawner.clone(),
        &mut tasks,
    );
    cron_service.ensure_system_cron_jobs().await;

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
        inbound_processor.clone(),
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
        inbound_processor.clone(),
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

    let mut slack_channel =
        gasket_engine::channels::slack::SlackChannel::new(slack_cfg, inbound_processor.clone());

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
    broker: &Arc<dyn gasket_engine::broker::MessageBroker>,
    workspace: &Path,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let heartbeat = gasket_engine::heartbeat::HeartbeatService::new(workspace.to_path_buf());
    let broker_for_heartbeat = broker.clone();
    tasks.push(tokio::spawn(async move {
        heartbeat
            .run(|task_text| {
                let broker_inner = broker_for_heartbeat.clone();
                async move {
                    let inbound = gasket_engine::channels::InboundMessage {
                        channel: gasket_engine::channels::ChannelType::Cli,
                        sender_id: "heartbeat".to_string(),
                        chat_id: "heartbeat".to_string(),
                        content: task_text,
                        media: None,
                        metadata: None,
                        timestamp: chrono::Utc::now(),
                        trace_id: None,
                    };
                    let envelope = gasket_engine::broker::Envelope::new(
                        gasket_engine::broker::Topic::Inbound,
                        &inbound,
                    );
                    let _ = broker_inner.publish(envelope).await;
                }
            })
            .await;
    }));
}

/// Start cron checker that polls for due jobs every 60 seconds.
/// Supports direct tool execution (bypassing LLM) for zero-token system tasks.
fn start_cron_checker(
    cron_service: &Arc<CronService>,
    broker: &Arc<dyn gasket_engine::broker::MessageBroker>,
    tools: Arc<ToolRegistry>,
    spawner: Arc<dyn SubagentSpawner>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let cron_svc = cron_service.clone();
    let broker_for_cron = broker.clone();
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
                            .unwrap_or(gasket_engine::channels::ChannelType::Cli);
                        let chat_id = job.chat_id.clone().unwrap_or_else(|| "cron".to_string());

                        // Check if this is a direct tool execution job (bypass LLM)
                        if let Some(ref tool_name) = job.tool {
                            // Direct tool execution path - ZERO LLM tokens consumed
                            tracing::info!(
                                "Executing cron job '{}' directly via tool '{}' (bypassing LLM)",
                                job.name,
                                tool_name
                            );

                            // Build ToolContext with broker-based outbound
                            let ctx = gasket_engine::tools::ToolContext::default()
                                .outbound_tx({
                                    // Create a temporary mpsc channel for tool output,
                                    // then forward to broker. This preserves the
                                    // ToolContext API while using broker underneath.
                                    let (tx, mut rx) = tokio::sync::mpsc::channel::<gasket_engine::channels::OutboundMessage>(16);
                                    let b = broker_for_cron.clone();
                                    tokio::spawn(async move {
                                        while let Some(msg) = rx.recv().await {
                                            let envelope = gasket_engine::broker::Envelope::new(
                                                gasket_engine::broker::Topic::Outbound,
                                                &msg,
                                            );
                                            let _ = b.publish(envelope).await;
                                        }
                                    });
                                    tx
                                })
                                .spawner(spawner.clone());

                            let args = job.tool_args.clone().unwrap_or(serde_json::json!({}));

                            // Execute tool directly
                            match tools.execute(tool_name, args, &ctx).await {
                                Ok(result) => {
                                    tracing::info!(
                                        "Cron job '{}' completed successfully.",
                                        job.name
                                    );
                                    tracing::info!("{}", result);
                                    // Send result to output channel
                                    let out_msg = gasket_engine::channels::OutboundMessage::new(
                                        channel, &chat_id, result,
                                    );
                                    let envelope = gasket_engine::broker::Envelope::new(
                                        gasket_engine::broker::Topic::Outbound,
                                        &out_msg,
                                    );
                                    let _ = broker_for_cron.publish(envelope).await;
                                }
                                Err(e) => {
                                    tracing::error!("Cron job '{}' failed: {}", job.name, e);
                                    // Send error to output channel
                                    let error_msg = format!("Cron job error: {}", e);
                                    let out_msg = gasket_engine::channels::OutboundMessage::new(
                                        channel, &chat_id, error_msg,
                                    );
                                    let envelope = gasket_engine::broker::Envelope::new(
                                        gasket_engine::broker::Topic::Outbound,
                                        &out_msg,
                                    );
                                    let _ = broker_for_cron.publish(envelope).await;
                                }
                            }
                        } else {
                            // Traditional LLM-based path
                            let inbound = gasket_engine::channels::InboundMessage {
                                channel,
                                sender_id: "cron".to_string(),
                                chat_id,
                                content: job.message.clone(),
                                media: None,
                                metadata: None,
                                timestamp: chrono::Utc::now(),
                                trace_id: None,
                            };
                            let envelope = gasket_engine::broker::Envelope::new(
                                gasket_engine::broker::Topic::Inbound,
                                &inbound,
                            );
                            let _ = broker_for_cron.publish(envelope).await;
                        }

                        // Advance job tick and persist state to database
                        // This ensures state survives restarts and missed ticks are handled
                        match cron_svc.advance_job_tick(&job.id).await {
                            Ok((last_run, next_run)) => {
                                tracing::debug!(
                                    "Advanced job {} tick: last_run={}, next_run={}",
                                    job.id, last_run, next_run
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to advance job {} tick: {}. Job may run again on next check.",
                                    job.id, e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get due cron jobs: {}", e);
                }
            }
        }
    }));
}
