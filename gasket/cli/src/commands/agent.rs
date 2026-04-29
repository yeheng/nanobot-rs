//! Agent 命令实现

use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};

use gasket_engine::channels::ChatEvent;
use gasket_engine::channels::SessionKey;
use gasket_engine::config::{load_config, ModelRegistry};
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::session::{AgentResponse, AgentSession};
use gasket_engine::subagents::SimpleSpawner;
use gasket_engine::token_tracker::ModelPricing;
use gasket_engine::ModelResolver;
use gasket_engine::SqliteStore;

use gasket_engine::broker::MemoryBroker;

use super::registry::CliModelResolver;
use crate::cli::AgentOptions;
use crate::provider::setup_vault;

/// Run the agent command
pub async fn cmd_agent(opts: AgentOptions) -> Result<()> {
    // Enable debug logging if requested
    if opts.logs {
        tracing_subscriber::fmt()
            .with_env_filter(Level::DEBUG.to_string())
            .try_init()
            .ok();
    }

    let config = load_config().await.context("Failed to load config")?;
    let workspace =
        gasket_engine::tools::resolve_exec_workspace(&config, std::path::Path::new("."));

    // ── Infrastructure initialization (explicit, once) ──
    gasket_engine::config::init_config(config.clone());
    let broker = Arc::new(MemoryBroker::new(256, 64));
    let sqlite_store = SqliteStore::new()
        .await
        .expect("Failed to open SqliteStore");
    gasket_storage::init_db(sqlite_store);
    let sqlite_store = Arc::new(gasket_storage::get_db().clone());

    // Check for vault placeholders and unlock if needed (JIT setup)
    let vault = setup_vault(&config)?;

    // Find a provider
    let provider_info = crate::provider::find_provider(&config, vault.as_deref())?;

    // Create agent config
    let mut agent_config = super::registry::build_agent_config(&config);
    agent_config.model = provider_info.model.clone();

    // Handle thinking mode
    if opts.thinking || agent_config.thinking_enabled {
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

    // Handle streaming mode
    if opts.no_stream {
        agent_config.streaming = false;
    }

    // Build tool registry (CLI mode: no bus/cron)
    let pool = sqlite_store.pool();

    // Initialize wiki stores if wiki config is enabled or wiki directory exists
    let wiki_root = workspace.join("wiki");
    let (page_store, page_index) =
        if wiki_root.exists() || agent_config.wiki.as_ref().map_or(false, |w| w.enabled) {
            use gasket_engine::wiki::{PageIndex, PageStore};
            use gasket_storage::wiki::TantivyPageIndex;
            let (wiki_changed_tx, mut wiki_changed_rx) = tokio::sync::mpsc::channel(64);
            let ps = PageStore::new(pool.clone(), wiki_root.clone())
                .with_wiki_changed_tx(wiki_changed_tx);
            let broker2 = broker.clone();
            tokio::spawn(async move {
                while let Some(path) = wiki_changed_rx.recv().await {
                    let envelope = gasket_engine::broker::Envelope::new(
                        gasket_engine::broker::Topic::WikiChanged,
                        gasket_engine::broker::BrokerPayload::WikiChanged { path },
                    );
                    let _ = broker2.try_publish(envelope);
                }
            });
            if let Err(e) = ps.init_dirs().await {
                tracing::warn!("Failed to init wiki dirs: {}", e);
            }
            if let Err(e) = gasket_engine::create_wiki_tables(&pool).await {
                tracing::warn!("Failed to create wiki tables: {}", e);
            }
            let tantivy_dir = wiki_root.join(".tantivy");
            let pi = match TantivyPageIndex::open(tantivy_dir) {
                Ok(idx) => Some(Arc::new(PageIndex::new(Arc::new(idx)))),
                Err(e) => {
                    tracing::warn!("Tantivy index open failed, search disabled: {}", e);
                    None
                }
            };
            (Some(ps), pi)
        } else {
            (None, None)
        };

    // Spawn wiki indexing service for auto Tantivy updates
    if let (Some(ref ps), Some(ref pi)) = (&page_store, &page_index) {
        let relation_store = gasket_storage::wiki::WikiRelationStore::new(pool.clone());
        let svc =
            gasket_engine::wiki::WikiIndexingService::new(ps.clone(), pi.clone(), relation_store);
        if let Ok(sub) = broker
            .subscribe(&gasket_engine::broker::Topic::WikiChanged)
            .await
        {
            let _ = svc.spawn(sub);
        }
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

    // Initialize embedding recall if configured
    #[cfg(feature = "embedding")]
    let (history_search, embedding_recall, event_store_tx) =
        if let Some(ref emb_cfg) = config.embedding {
            let event_store = gasket_engine::EventStore::new(sqlite_store.pool());
            let tx = Some(event_store.sender());
            match gasket_engine::session::history::builder::setup_embedding_recall(
                &event_store,
                emb_cfg,
            )
            .await
            {
                Ok((searcher, indexer)) => {
                    let params = gasket_engine::tools::HistorySearchParams {
                        searcher: searcher.clone(),
                        config: emb_cfg.recall.clone(),
                    };
                    (Some(params), Some((searcher, indexer)), tx)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize embedding recall: {}", e);
                    (None, None, None)
                }
            }
        } else {
            (None, None, None)
        };
    // (non-embedding builds skip semantic recall initialization)

    // Build common tool registry once and share it between agent and subagent
    let common_tools =
        gasket_engine::tools::build_tool_registry(gasket_engine::tools::ToolRegistryConfig {
            subagent_spawner: None,
            extra_tools: vec![],
            page_store,
            page_index,
            provider: Some(provider_info.provider.clone()),
            model: Some(provider_info.model.clone()),
            #[cfg(feature = "embedding")]
            history_search,
        });

    let tools = Arc::new(common_tools.clone());

    // Create spawner for spawn/spawn_parallel tools
    let subagent_tools = Arc::new(common_tools.clone());

    // Create model resolver for subagent spawner to support model_id switching in spawn tools
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

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    #[cfg(feature = "embedding")]
    let agent = if let Some((searcher, indexer)) = embedding_recall {
        AgentSession::with_sqlite_store_and_embedding(
            provider_info.provider,
            workspace,
            agent_config,
            tools,
            sqlite_store,
            gasket_engine::session::builder::EmbeddingContext {
                searcher,
                indexer,
                event_store_tx,
            },
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?
        .with_pricing(pricing)
        .with_spawner(subagent_spawner)
    } else {
        AgentSession::with_sqlite_store(
            provider_info.provider,
            workspace,
            agent_config,
            tools,
            sqlite_store,
        )
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?
        .with_pricing(pricing)
        .with_spawner(subagent_spawner)
    };
    #[cfg(not(feature = "embedding"))]
    let agent = AgentSession::with_sqlite_store(
        provider_info.provider,
        workspace,
        agent_config,
        tools,
        sqlite_store,
    )
    .await
    .context("Failed to initialize agent (check workspace bootstrap files)")?
    .with_pricing(pricing)
    .with_spawner(subagent_spawner);

    let render_md = !opts.no_markdown;
    let use_streaming = !opts.no_stream;

    match opts.message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let session_key = SessionKey::new(gasket_engine::channels::ChannelType::Cli, "direct");
            if use_streaming {
                // Use channel-based streaming API
                let (mut event_rx, result_handle) = agent
                    .process_direct_streaming_with_channel(&msg, &session_key)
                    .await?;

                // Forward events to callback
                let forward_handle = tokio::spawn(async move {
                    while let Some(event) = event_rx.recv().await {
                        match event {
                            ChatEvent::Content { content } => print!("{}", content),
                            ChatEvent::Thinking { content } => {
                                eprint!("{}", content.dimmed().italic());
                                std::io::stderr().flush().ok();
                            }
                            ChatEvent::Done => println!(),
                            // TokenStats are handled internally; skip other non-user events
                            _ => {}
                        }
                    }
                });

                // Wait for streaming to complete
                let (result, _) = tokio::join!(result_handle, forward_handle);
                if let Err(e) = result {
                    return Err(anyhow::anyhow!("Task join error: {}", e));
                }
            } else {
                let response = agent.process_direct(&msg, &session_key).await?;
                print_response_with_reasoning(&response, render_md);
            }
        }
        None => {
            // Interactive mode
            println!("🐈 gasket interactive mode. Type '/help' for commands, '/exit' to quit.\n");

            let mut line_editor = Reedline::create();
            let prompt =
                DefaultPrompt::new(DefaultPromptSegment::Empty, DefaultPromptSegment::Empty);

            let interactive_session =
                SessionKey::new(gasket_engine::channels::ChannelType::Cli, "interactive");

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

                        // Handle CLI-specific slash commands locally
                        // (these must NOT reach the agent core — other channels
                        //  like Telegram should treat "/new" as normal LLM input)
                        let cmd = line.to_lowercase();
                        if cmd == "/new" {
                            agent.clear_session(&interactive_session).await;
                            println!("New session started.");
                            continue;
                        }
                        if cmd == "/help" {
                            println!(
                                "🐈 gasket commands:\n\
                                 /new  — Start a new conversation\n\
                                 /help — Show available commands\n\
                                 /exit — Exit the REPL"
                            );
                            continue;
                        }

                        // Process the message
                        if use_streaming {
                            println!();
                            // Use channel-based streaming API
                            let streaming_result = agent
                                .process_direct_streaming_with_channel(line, &interactive_session)
                                .await;

                            match streaming_result {
                                Ok((mut event_rx, result_handle)) => {
                                    // Forward events to callback
                                    let forward_handle = tokio::spawn(async move {
                                        while let Some(event) = event_rx.recv().await {
                                            match event {
                                                ChatEvent::Content { content } => {
                                                    print!("{}", content);
                                                    std::io::stdout().flush().ok();
                                                }
                                                ChatEvent::Thinking { content } => {
                                                    eprint!("{}", content.dimmed().italic());
                                                    std::io::stderr().flush().ok();
                                                }
                                                ChatEvent::Done => {}
                                                _ => {}
                                            }
                                        }
                                    });

                                    // Wait for streaming to complete
                                    let (result, _) = tokio::join!(result_handle, forward_handle);
                                    if result.is_ok() {
                                        println!("\n");
                                    } else if let Err(e) = result {
                                        println!("\n{} {}\n", "Error:".red(), e);
                                    }
                                }
                                Err(e) => {
                                    println!("\n{} {}\n", "Error:".red(), e);
                                }
                            }
                        } else {
                            match agent.process_direct(line, &interactive_session).await {
                                Ok(response) => {
                                    println!();
                                    print_response_with_reasoning(&response, render_md);
                                    println!();
                                }
                                Err(e) => println!("\n{} {}\n", "Error:".red(), e),
                            }
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
