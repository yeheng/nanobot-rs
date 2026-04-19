//! TUI command — starts a lightweight gateway with only the TUI channel.
//!
//! Instead of interacting directly with AgentSession, the TUI runs as a
//! first-class channel (gasket-channels::tui) plugged into the broker pipeline.

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use gasket_engine::broker::{MemoryBroker, SessionManager};
use gasket_engine::bus_adapter::EngineHandler;
use gasket_engine::config::{load_config, ModelRegistry};
use gasket_engine::memory::MemoryStore;
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::session::AgentSession;
use gasket_engine::subagents::SimpleSpawner;
use gasket_engine::token_tracker::ModelPricing;
use gasket_engine::tools::{build_tool_registry, ToolRegistryConfig};
use gasket_engine::ModelResolver;
use gasket_engine::OutboundDispatcher;

use super::registry::CliModelResolver;
use crate::cli::TuiOptions;
use crate::provider::setup_vault;

/// Run the TUI command — a mini-gateway with only the TUI channel enabled.
pub async fn cmd_tui(_opts: TuiOptions) -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;
    let vault = setup_vault(&config)?;

    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".gasket");

    println!("🐈 Starting TUI chat...\n");

    // --- Broker ---
    let broker: Arc<MemoryBroker> = Arc::new(MemoryBroker::new(1024, 256));

    // --- Agent ---
    let provider_info = crate::provider::find_provider(&config, vault.as_deref())?;
    let mut agent_config = super::registry::build_agent_config(&config);
    agent_config.model = provider_info.model.clone();

    if agent_config.thinking_enabled && !provider_info.supports_thinking {
        tracing::warn!(
            "Provider '{}' does not support thinking mode. Thinking disabled.",
            provider_info.provider_name
        );
        agent_config.thinking_enabled = false;
    }

    let memory_store = Arc::new(MemoryStore::new().await);
    let sqlite_store = memory_store.sqlite_store().clone();

    let common_tools = build_tool_registry(ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_spawner: None,
        extra_tools: vec![],
        sqlite_store: None,
    });

    let mut subagent_tools = common_tools.clone();
    let subagent_tools_arc = Arc::new(subagent_tools.clone());
    subagent_tools.link_engine_refs(subagent_tools_arc, provider_info.provider.clone());
    let subagent_tools = Arc::new(subagent_tools);

    let mut resolver_registry = ProviderRegistry::from_config(&config);
    if let Some(ref v) = vault {
        resolver_registry.with_vault(v.clone());
    }
    let model_resolver: Arc<dyn ModelResolver> = Arc::new(CliModelResolver {
        provider_registry: resolver_registry,
        model_registry: ModelRegistry::from_config(&config.agents),
    });

    let subagent_spawner: Arc<dyn gasket_engine::SubagentSpawner> = Arc::new(
        SimpleSpawner::new(
            provider_info.provider.clone(),
            subagent_tools,
            workspace.clone(),
        )
        .with_model_resolver(model_resolver),
    );

    let mut tools = common_tools.clone();
    gasket_engine::tools::register_sqlite_tools(&mut tools, &sqlite_store);
    let tools_arc = Arc::new(tools.clone());
    tools.link_engine_refs(tools_arc, provider_info.provider.clone());
    let tools = Arc::new(tools);

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
        .context("Failed to initialize agent")?
        .with_spawner(subagent_spawner),
    );

    // --- Providers (only TUI) ---
    let inbound_sender = gasket_engine::channels::InboundSender::new_with_broker(broker.clone());
    let mut providers =
        gasket_engine::channels::ImProviders::from_config(&config.channels, inbound_sender.clone());

    // Inject TUI adapter if not already present
    let has_tui = providers.iter().any(|p| p.name() == "tui");
    if !has_tui {
        providers.push(gasket_engine::channels::ImProvider::Tui(
            gasket_engine::channels::tui::TuiAdapter::new(),
        ));
    }

    let providers = Arc::new(providers);

    // --- Background tasks ---
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Outbound dispatcher
    let outbound_dispatcher = OutboundDispatcher::new(broker.clone(), providers.clone());
    tasks.push(tokio::spawn(outbound_dispatcher.run()));

    // Session manager
    {
        let handler = Arc::new(EngineHandler::new(agent));
        let session_mgr = SessionManager::new(
            broker.clone(),
            handler,
            std::time::Duration::from_secs(3600),
        );
        tasks.push(tokio::spawn(session_mgr.run()));
    }

    // Start TUI adapter (blocks until user exits)
    let tui_provider = providers.iter().find(|p| p.name() == "tui").unwrap();
    info!("Starting TUI adapter...");
    let tui_result = tui_provider.start(inbound_sender.clone()).await;

    println!("\n🐈 Shutting down...");

    // Abort background tasks
    for task in &tasks {
        task.abort();
    }
    use tokio::time::{timeout, Duration};
    for task in tasks {
        let _ = timeout(Duration::from_millis(500), task).await;
    }

    tui_result
}
