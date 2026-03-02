//! Gateway 命令实现

use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;

use nanobot_core::agent::{AgentConfig, AgentLoop, SubagentManager};
use nanobot_core::config::load_config;
use nanobot_core::cron::CronService;
use nanobot_core::tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MessageTool, ReadFileTool, SpawnTool,
    ToolMetadata, ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};
use nanobot_core::{Config, Tool};
use tokio::sync::mpsc::Sender;

/// Run the gateway command
pub async fn cmd_gateway() -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;
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
    let (bus, inbound_rx, outbound_rx) = nanobot_core::bus::MessageBus::new(100);
    let bus = Arc::new(bus);

    // Create cron service
    let cron_service = Arc::new(CronService::new(workspace.clone()).await);

    // Create agent with all dependencies
    let provider_info = crate::provider::find_provider(&config)?;
    let mut agent_config = build_agent_config(&config);
    agent_config.model = provider_info.model.clone();

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

    let subagent_manager = Arc::new(
        SubagentManager::new(
            provider_info.provider.clone(),
            workspace.clone(),
            Arc::new({
                let cfg = config.clone();
                let ws = workspace.clone();
                let cron_svc = cron_service.clone();
                let ob_tx = bus.outbound_sender();
                move || {
                    build_tool_registry(&cfg, &ws, cron_svc.clone(), vec![], None, ob_tx.clone())
                }
            }),
            bus.outbound_sender(),
        )
        .await,
    );

    // Build tool registry externally
    let tools = build_tool_registry(
        &config,
        &workspace,
        cron_service.clone(),
        mcp_tools,
        Some(subagent_manager),
        bus.outbound_sender(),
    );

    let agent = Arc::new(
        AgentLoop::new(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?,
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Build InboundSender with auth/rate-limit middleware applied.
    let inbound_sender = nanobot_core::channels::InboundSender::new(bus.inbound_sender());
    // TODO: wire up rate_limiter and auth_checker from config if needed
    #[allow(unused_variables)]
    let inbound_processor = inbound_sender.clone();

    // --- Actor Pipeline ---
    // Router Actor owns the session table (plain HashMap, zero locks).
    // Session Actors serialize per-session processing via dedicated mpsc channels.
    // Outbound Actor decouples network I/O from the agent loop.

    // 1. Start Outbound Actor (consumes outbound_rx, fire-and-forget HTTP sends)
    let channels_config = Arc::new(config.channels.clone());
    tasks.push(tokio::spawn(nanobot_core::bus::run_outbound_actor(
        outbound_rx,
        channels_config,
    )));

    // 2. Start Router Actor (dispatches inbound to per-session channels)
    {
        let outbound_tx = bus.outbound_sender();
        let agent_for_router = agent.clone();
        tasks.push(tokio::spawn(nanobot_core::bus::run_router_actor(
            inbound_rx,
            outbound_tx,
            agent_for_router,
        )));
    }

    // --- Heartbeat service ---
    {
        let heartbeat = nanobot_core::heartbeat::HeartbeatService::new(workspace.clone());
        let bus_for_heartbeat = bus.clone();
        tasks.push(tokio::spawn(async move {
            heartbeat
                .run(|task_text| {
                    let bus_inner = bus_for_heartbeat.clone();
                    async move {
                        let inbound = nanobot_core::bus::events::InboundMessage {
                            channel: nanobot_core::bus::ChannelType::Cli,
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
                                .unwrap_or(nanobot_core::bus::ChannelType::Cli);
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
                    Err(e) => {
                        tracing::error!("Failed to get due cron jobs: {}", e);
                    }
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
                inbound_processor.raw_sender(),
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
                inbound_processor.raw_sender(),
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
            use nanobot_core::channels::Channel;
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

    // Start Email if configured
    #[cfg(feature = "email")]
    if let Some(email_config) = &config.channels.email {
        if email_config.enabled {
            println!("{} Email channel", "✓".green());

            // Check if required fields are present
            let has_imap = email_config.imap_host.is_some()
                && email_config.imap_username.is_some()
                && email_config.imap_password.is_some();
            let has_smtp = email_config.smtp_host.is_some()
                && email_config.smtp_username.is_some()
                && email_config.smtp_password.is_some()
                && email_config.from_address.is_some();

            if has_imap || has_smtp {
                let email_cfg = nanobot_core::channels::email::EmailConfig {
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

                let email_channel = nanobot_core::channels::email::EmailChannel::new(
                    email_cfg,
                    inbound_processor.raw_sender(),
                );

                let task = tokio::spawn(async move {
                    let _ = email_channel.start_polling().await;
                });

                tasks.push(task);
            }
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
        streaming: config.agents.defaults.streaming,
    }
}

/// Build tool registry for gateway mode
fn build_tool_registry(
    config: &Config,
    workspace: &std::path::Path,
    cron_service: Arc<CronService>,
    mcp_tools: Vec<Box<dyn Tool>>,
    subagent_manager: Option<Arc<SubagentManager>>,
    outbound_tx: Sender<nanobot_core::bus::OutboundMessage>,
) -> ToolRegistry {
    let restrict = config.tools.restrict_to_workspace;
    let allowed_dir = if restrict {
        Some(workspace.to_path_buf())
    } else {
        None
    };

    // Resolve exec workspace directory
    let exec_workspace = resolve_exec_workspace(config, workspace);

    let mut tools = ToolRegistry::new();

    // Safe read-only tools (no approval required)
    tools.register_with_metadata(
        Box::new(ReadFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Read File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["read".to_string(), "file".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(ListDirTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "List Directory".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["read".to_string(), "directory".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(WebFetchTool::new()),
        ToolMetadata {
            display_name: "Web Fetch".to_string(),
            category: "web".to_string(),
            tags: vec!["http".to_string(), "fetch".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(WebSearchTool::new(Some(config.tools.web.clone()))),
        ToolMetadata {
            display_name: "Web Search".to_string(),
            category: "web".to_string(),
            tags: vec!["search".to_string(), "web".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // Dangerous mutating tools (require approval)
    tools.register_with_metadata(
        Box::new(WriteFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Write File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["write".to_string(), "file".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );
    tools.register_with_metadata(
        Box::new(EditFileTool::new(allowed_dir.clone())),
        ToolMetadata {
            display_name: "Edit File".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["edit".to_string(), "file".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );
    tools.register_with_metadata(
        Box::new(ExecTool::from_config(
            exec_workspace,
            &config.tools.exec,
            restrict,
        )),
        ToolMetadata {
            display_name: "Execute Command".to_string(),
            category: "system".to_string(),
            tags: vec!["shell".to_string(), "exec".to_string()],
            requires_approval: true,
            is_mutating: true,
        },
    );
    let spawn_tool = match subagent_manager {
        Some(mgr) => SpawnTool::with_manager(mgr),
        None => SpawnTool::new(),
    };
    tools.register_with_metadata(
        Box::new(spawn_tool),
        ToolMetadata {
            display_name: "Spawn Subagent".to_string(),
            category: "system".to_string(),
            tags: vec!["spawn".to_string(), "agent".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // Communication tools (gateway-specific)
    tools.register_with_metadata(
        Box::new(MessageTool::new(outbound_tx)),
        ToolMetadata {
            display_name: "Send Message".to_string(),
            category: "communication".to_string(),
            tags: vec!["message".to_string(), "send".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );
    tools.register_with_metadata(
        Box::new(CronTool::new(cron_service.clone())),
        ToolMetadata {
            display_name: "Schedule Task".to_string(),
            category: "system".to_string(),
            tags: vec!["cron".to_string(), "schedule".to_string()],
            requires_approval: false,
            is_mutating: false,
        },
    );

    // MCP tools (metadata assigned by MCP manager)
    for mcp_tool in mcp_tools {
        tools.register(mcp_tool);
    }

    tools
}

/// Resolve the exec workspace directory from config or default to $HOME/.nanobot.
///
/// Creates the directory if it doesn't exist.
fn resolve_exec_workspace(config: &Config, fallback: &std::path::Path) -> std::path::PathBuf {
    let workspace_path = if let Some(ref ws) = config.tools.exec.workspace {
        std::path::PathBuf::from(ws)
    } else {
        // Default: $HOME/.nanobot
        dirs::home_dir()
            .map(|h| h.join(".nanobot"))
            .unwrap_or_else(|| fallback.to_path_buf())
    };

    // Ensure the directory exists
    if !workspace_path.exists() {
        if let Err(e) = std::fs::create_dir_all(&workspace_path) {
            tracing::warn!(
                "Failed to create exec workspace {:?}: {}. Falling back to {:?}",
                workspace_path,
                e,
                fallback
            );
            return fallback.to_path_buf();
        }
        tracing::info!("Created exec workspace: {:?}", workspace_path);
    }

    workspace_path
}
