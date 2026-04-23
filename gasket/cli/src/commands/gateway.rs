//! Gateway 命令实现

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::info;

use gasket_engine::bus_adapter::EngineHandler;

use gasket_engine::config::{load_config, ModelRegistry};
use gasket_engine::cron::CronService;
use gasket_engine::memory::EventStore;
use gasket_engine::memory::MemoryStore;
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::session::{AgentSession, ContextCompactor};
use gasket_engine::subagents::SimpleSpawner;
use gasket_engine::token_tracker::ModelPricing;
use gasket_engine::tools::ContextTool;
use gasket_engine::tools::WikiDecayTool;
use gasket_engine::tools::{build_tool_registry, CronTool, Tool, ToolContext, ToolRegistryConfig};
use gasket_engine::tools::{MessageTool, ToolMetadata, ToolRegistry};
use gasket_engine::SubagentSpawner;

use gasket_engine::broker::{BrokerPayload, MemoryBroker, SessionManager};
use gasket_engine::OutboundDispatcher;
use gasket_types::SessionKey;

use super::registry::CliModelResolver;
use crate::provider::setup_vault;
use axum::response::IntoResponse;
use tower_http::cors::CorsLayer;

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
    let has_channels = config.channels.enabled_count() > 0;

    if !has_channels {
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
    let broker: Arc<MemoryBroker> = Arc::new(MemoryBroker::new(1024, 256));

    // MemoryStore provides the underlying SqliteStore for session management
    let memory_store = Arc::new(MemoryStore::new().await);

    // Initialize wiki stores if wiki directory exists
    let pool = memory_store.sqlite_store().pool();
    let wiki_root = workspace.join("wiki");
    let (page_store, page_index) = if wiki_root.exists() {
        let ps = Arc::new(gasket_engine::wiki::PageStore::new(
            pool.clone(),
            wiki_root.clone(),
        ));
        if let Err(e) = ps.init_dirs().await {
            tracing::warn!("Failed to init wiki dirs: {}", e);
        }
        let tantivy_dir = wiki_root.join(".tantivy");
        let pi = match gasket_engine::wiki::PageIndex::open(tantivy_dir) {
            Ok(idx) => Some(Arc::new(idx)),
            Err(e) => {
                tracing::warn!("Tantivy index open failed, search disabled: {}", e);
                None
            }
        };
        (Some(ps), pi)
    } else {
        (None, None)
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
    let _provider_registry = Arc::new(provider_registry);

    // Log available models if any are configured
    if !model_registry.is_empty() {
        info!(
            "Model switching enabled with {} model profiles: {}",
            model_registry.len(),
            model_registry.list_available_models().join(", ")
        );
    }

    // Build common tool registry once and share it between agent and subagent
    let common_tools = build_tool_registry(ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_spawner: None,
        extra_tools: vec![],
        sqlite_store: Some(memory_store.sqlite_store().clone()),
        page_store: page_store.clone(),
        page_index: page_index.clone(),
        provider: Some(provider_info.provider.clone()),
        model: Some(provider_info.model.clone()),
    });

    let mut subagent_tools = common_tools.clone();
    let subagent_tools_arc = Arc::new(subagent_tools.clone());
    subagent_tools.link_engine_refs(subagent_tools_arc, provider_info.provider.clone());
    let subagent_tools = Arc::new(subagent_tools);

    let subagent_spawner: Arc<dyn SubagentSpawner> = Arc::new(
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

    let extra_tools: Vec<(Box<dyn Tool>, ToolMetadata)> = {
        let mut ext = vec![(
            Box::new(MessageTool::new_broker(broker.clone())) as Box<dyn Tool>,
            ToolMetadata {
                display_name: "Send Message".to_string(),
                category: "communication".to_string(),
                tags: vec!["message".to_string(), "send".to_string()],
                requires_approval: false,
                is_mutating: false,
            },
        )];

        ext.push((
            Box::new(CronTool::new(cron_service.clone())) as Box<dyn Tool>,
            ToolMetadata {
                display_name: "Schedule Task".to_string(),
                category: "system".to_string(),
                tags: vec!["cron".to_string(), "schedule".to_string()],
                requires_approval: false,
                is_mutating: false,
            },
        ));

        // Wiki tools (wiki-only) - only add if page_store is available
        if let Some(ref ps) = page_store {
            if let Some(ref pi) = page_index {
                ext.push((
                    Box::new(gasket_engine::tools::WikiRefreshTool::new(
                        ps.clone(),
                        pi.clone(),
                    )) as Box<dyn Tool>,
                    ToolMetadata {
                        display_name: "Wiki Refresh".to_string(),
                        category: "system".to_string(),
                        tags: vec![
                            "wiki".to_string(),
                            "refresh".to_string(),
                            "index".to_string(),
                        ],
                        requires_approval: false,
                        is_mutating: true,
                    },
                ));
            }

            ext.push((
                Box::new(WikiDecayTool::new(ps.clone())) as Box<dyn Tool>,
                ToolMetadata {
                    display_name: "Wiki Decay".to_string(),
                    category: "system".to_string(),
                    tags: vec![
                        "wiki".to_string(),
                        "decay".to_string(),
                        "maintenance".to_string(),
                    ],
                    requires_approval: false,
                    is_mutating: true,
                },
            ));
        }

        // Context management tool — uses the same SqliteStore as the session
        let ctx_sqlite = Arc::new(memory_store.sqlite_store().clone());
        let ctx_event_store = Arc::new(EventStore::new(ctx_sqlite.pool()));
        let mut ctx_compactor = ContextCompactor::new(
            provider_info.provider.clone(),
            ctx_event_store,
            ctx_sqlite,
            provider_info.model.clone(),
            8000, // HistoryConfig::default().token_budget
        );
        if let Some(ref prompt) = agent_config.summarization_prompt {
            ctx_compactor = ctx_compactor.with_summarization_prompt(prompt.clone());
        }
        ext.push((
            Box::new(ContextTool::new(Arc::new(ctx_compactor))) as Box<dyn Tool>,
            ToolMetadata {
                display_name: "Context Management".to_string(),
                category: "system".to_string(),
                tags: vec!["context".to_string(), "compression".to_string()],
                requires_approval: false,
                is_mutating: true,
            },
        ));

        ext
    };

    let mut tools = common_tools.clone();
    for (tool, metadata) in extra_tools {
        tools.register_with_metadata(tool, metadata);
    }
    let tools_arc = Arc::new(tools.clone());
    tools.link_engine_refs(tools_arc, provider_info.provider.clone());
    let tools = Arc::new(tools);

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    let agent = Arc::new(
        AgentSession::with_memory_store(
            provider_info.provider,
            workspace.clone(),
            agent_config,
            tools.clone(),
            memory_store,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?
        .with_pricing(pricing)
        .with_spawner(subagent_spawner.clone()),
    );

    // Track running tasks
    #[allow(unused_mut)]
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Build InboundSender with broker mode (replaces bus.inbound_sender()).
    let inbound_sender = gasket_engine::channels::InboundSender::new_with_broker(broker.clone());
    // TODO: wire up rate_limiter and auth_checker from config if needed
    #[allow(unused_variables)]
    let inbound_processor = inbound_sender.clone();

    // --- Actor Pipeline ---
    // Router Actor owns the session table (plain HashMap, zero locks).
    // Session Actors serialize per-session processing via dedicated mpsc channels.
    // Outbound Actor decouples network I/O from the agent loop.

    // Build IM providers (replaces OutboundSenderRegistry + ChannelRegistry)
    let providers =
        gasket_engine::channels::ImProviders::from_config(&config.channels, inbound_sender.clone());

    let providers = Arc::new(providers);

    // Start HTTP server for webhook-based providers (WebSocket, DingTalk, Feishu, WeCom)
    #[cfg(any(
        feature = "websocket",
        feature = "dingtalk",
        feature = "feishu",
        feature = "wecom"
    ))]
    {
        let providers_for_http = providers.clone();
        let agent_for_http = agent.clone();
        tasks.push(tokio::spawn(async move {
            let mut app = axum::Router::new();

            // Merge routes from enabled providers
            for provider in providers_for_http.iter() {
                if let Some(router) = provider.routes() {
                    app = app.merge(router);
                }
            }

            // Add context management HTTP endpoints
            let agent_for_context = agent_for_http.clone();
            let agent_for_compact = agent_for_http.clone();
            app = app
                .route(
                    "/api/sessions/{session_key}/context",
                    axum::routing::get(move |axum::extract::Path(session_key): axum::extract::Path<String>| {
                        let agent = agent_for_context.clone();
                        async move {
                            let key = match SessionKey::parse(&session_key) {
                                Some(k) => k,
                                None => {
                                    return (
                                        axum::http::StatusCode::BAD_REQUEST,
                                        axum::Json(serde_json::json!({"error": "Invalid session key"})),
                                    )
                                        .into_response();
                                }
                            };

                            match (
                                agent.get_context_stats(&key).await,
                                agent.get_watermark_info(&key).await,
                            ) {
                                (Some(stats), Some(watermark)) => {
                                    let body = serde_json::json!({
                                        "context_stats": {
                                            "token_budget": stats.token_budget,
                                            "compaction_threshold": stats.compaction_threshold,
                                            "threshold_tokens": stats.threshold_tokens,
                                            "current_tokens": stats.current_tokens,
                                            "usage_percent": stats.usage_percent,
                                            "is_compressing": stats.is_compressing,
                                        },
                                        "watermark_info": {
                                            "watermark": watermark.watermark,
                                            "max_sequence": watermark.max_sequence,
                                            "uncompacted_count": watermark.uncompacted_count,
                                            "compacted_percent": watermark.compacted_percent,
                                        }
                                    });
                                    (axum::http::StatusCode::OK, axum::Json(body)).into_response()
                                }
                                _ => {
                                    (
                                        axum::http::StatusCode::NOT_FOUND,
                                        axum::Json(serde_json::json!({"error": "Session not found or no compactor available"})),
                                    )
                                        .into_response()
                                }
                            }
                        }
                    })
                )
                .route(
                    "/api/sessions/{session_key}/context/compact",
                    axum::routing::post(move |axum::extract::Path(session_key): axum::extract::Path<String>| {
                        let agent = agent_for_compact.clone();
                        async move {
                            let key = match SessionKey::parse(&session_key) {
                                Some(k) => k,
                                None => {
                                    return (
                                        axum::http::StatusCode::BAD_REQUEST,
                                        axum::Json(serde_json::json!({"error": "Invalid session key"})),
                                    )
                                        .into_response();
                                }
                            };

                            match agent.force_compact_and_wait(&key, &[]).await {
                                Ok(()) => {
                                    match (
                                        agent.get_context_stats(&key).await,
                                        agent.get_watermark_info(&key).await,
                                    ) {
                                        (Some(stats), Some(watermark)) => {
                                            let body = serde_json::json!({
                                                "status": "compaction_completed",
                                                "context_stats": {
                                                    "token_budget": stats.token_budget,
                                                    "compaction_threshold": stats.compaction_threshold,
                                                    "threshold_tokens": stats.threshold_tokens,
                                                    "current_tokens": stats.current_tokens,
                                                    "usage_percent": stats.usage_percent,
                                                    "is_compressing": stats.is_compressing,
                                                },
                                                "watermark_info": {
                                                    "watermark": watermark.watermark,
                                                    "max_sequence": watermark.max_sequence,
                                                    "uncompacted_count": watermark.uncompacted_count,
                                                    "compacted_percent": watermark.compacted_percent,
                                                }
                                            });
                                            (axum::http::StatusCode::OK, axum::Json(body)).into_response()
                                        }
                                        _ => {
                                            (
                                                axum::http::StatusCode::OK,
                                                axum::Json(serde_json::json!({ "status": "compaction_completed" })),
                                            )
                                                .into_response()
                                        }
                                    }
                                }
                                Err(e) => {
                                    (
                                        axum::http::StatusCode::CONFLICT,
                                        axum::Json(serde_json::json!({ "error": e.to_string() })),
                                    )
                                        .into_response()
                                }
                            }
                        }
                    }),
                );

            app = app.layer(CorsLayer::permissive());

            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
            tracing::info!("HTTP server listening on {}", addr);
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    if let Err(e) = axum::serve(listener, app).await {
                        tracing::error!("HTTP server error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to bind HTTP server port 3000: {}", e);
                }
            }
        }));
    }

    // --- Broker Pipeline (replaces old Router/Session/Outbound actors) ---

    // 1. Start OutboundDispatcher (subscribes to Topic::Outbound, routes to providers)
    let outbound_dispatcher = OutboundDispatcher::new(broker.clone(), providers.clone());
    tasks.push(tokio::spawn(outbound_dispatcher.run()));

    // 2. Start SessionManager (subscribes to Topic::Inbound, dispatches to per-session tasks)
    {
        let handler = Arc::new(EngineHandler::new(agent));
        let session_mgr = SessionManager::new(
            broker.clone(),
            handler,
            std::time::Duration::from_secs(3600),
        );
        tasks.push(tokio::spawn(session_mgr.run()));
    }

    // --- Background services ---
    start_heartbeat_service(&broker, &workspace, &mut tasks);
    // Ensure system cron jobs exist before starting the cron checker
    // This prevents "Job not found" errors when advancing ticks for jobs
    // that exist in the database but not yet in memory
    cron_service.ensure_system_cron_jobs().await;
    start_cron_checker(
        &cron_service,
        &broker,
        tools.clone(),
        subagent_spawner.clone(),
        &mut tasks,
    );

    // --- Start all configured adapters ---
    let adapter_tasks = providers.spawn_all(&inbound_processor);
    tasks.extend(adapter_tasks);

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

/// Start heartbeat service that periodically sends heartbeat tasks through the bus.
fn start_heartbeat_service(
    broker: &Arc<MemoryBroker>,
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
                        BrokerPayload::Inbound(inbound),
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
    broker: &Arc<MemoryBroker>,
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
            let due = cron_svc.get_due_jobs();
            for job in due {
                tracing::info!("Cron job due: {} ({})", job.name, job.id);

                let channel = job
                    .channel
                    .as_deref()
                    .and_then(|c| serde_json::from_value(serde_json::json!(c)).ok())
                    .unwrap_or(gasket_engine::channels::ChannelType::Cli);
                let chat_id = job.chat_id.clone().unwrap_or_else(|| "cron".to_string());
                let is_broadcast = chat_id == "*";

                // Check if this is a direct tool execution job (bypass LLM)
                if let Some(ref tool_name) = job.tool {
                    // Direct tool execution path - ZERO LLM tokens consumed
                    tracing::info!(
                        "Executing cron job '{}' directly via tool '{}' (bypassing LLM)",
                        job.name,
                        tool_name
                    );

                    // Build ToolContext with broker-based outbound
                    let ctx = ToolContext::default()
                        .outbound_tx({
                            // Create a temporary mpsc channel for tool output,
                            // then forward to broker. This preserves the
                            // ToolContext API while using broker underneath.
                            let (tx, mut rx) = tokio::sync::mpsc::channel::<
                                gasket_engine::channels::OutboundMessage,
                            >(16);
                            let b = broker_for_cron.clone();
                            tokio::spawn(async move {
                                while let Some(msg) = rx.recv().await {
                                    let envelope = gasket_engine::broker::Envelope::new(
                                        gasket_engine::broker::Topic::Outbound,
                                        BrokerPayload::Outbound(msg),
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
                            tracing::info!("Cron job '{}' completed successfully.", job.name);
                            tracing::info!("{}", result);
                            // Send result to output channel
                            let out_msg = if is_broadcast {
                                gasket_engine::channels::OutboundMessage::broadcast(channel, result)
                            } else {
                                gasket_engine::channels::OutboundMessage::new(
                                    channel, &chat_id, result,
                                )
                            };
                            let envelope = gasket_engine::broker::Envelope::new(
                                gasket_engine::broker::Topic::Outbound,
                                BrokerPayload::Outbound(out_msg),
                            );
                            let _ = broker_for_cron.publish(envelope).await;
                        }
                        Err(e) => {
                            tracing::error!("Cron job '{}' failed: {}", job.name, e);
                            // Send error to output channel
                            let error_msg = format!("Cron job error: {}", e);
                            let out_msg = if is_broadcast {
                                gasket_engine::channels::OutboundMessage::broadcast(
                                    channel, error_msg,
                                )
                            } else {
                                gasket_engine::channels::OutboundMessage::new(
                                    channel, &chat_id, error_msg,
                                )
                            };
                            let envelope = gasket_engine::broker::Envelope::new(
                                gasket_engine::broker::Topic::Outbound,
                                BrokerPayload::Outbound(out_msg),
                            );
                            let _ = broker_for_cron.publish(envelope).await;
                        }
                    }
                } else if is_broadcast {
                    // Broadcast path: send the message directly to all connected clients
                    let out_msg = gasket_engine::channels::OutboundMessage::broadcast(
                        channel,
                        job.message.clone(),
                    );
                    let envelope = gasket_engine::broker::Envelope::new(
                        gasket_engine::broker::Topic::Outbound,
                        BrokerPayload::Outbound(out_msg),
                    );
                    let _ = broker_for_cron.publish(envelope).await;
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
                        BrokerPayload::Inbound(inbound),
                    );
                    let _ = broker_for_cron.publish(envelope).await;
                }

                // Advance job tick and persist state to database
                // This ensures state survives restarts and missed ticks are handled
                match cron_svc.advance_job_tick(&job.id).await {
                    Ok((last_run, next_run)) => {
                        tracing::debug!(
                            "Advanced job {} tick: last_run={}, next_run={}",
                            job.id,
                            last_run,
                            next_run
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to advance job {} tick: {}. Job may run again on next check.",
                            job.id,
                            e
                        );
                    }
                }
            }
        }
    }));
}
