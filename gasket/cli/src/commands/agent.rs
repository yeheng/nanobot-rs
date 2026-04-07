//! Agent 命令实现

use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};

use gasket_engine::agent::memory::MemoryStore;
use gasket_engine::agent::{AgentLoop, AgentResponse, ModelResolver, StreamEvent, SubagentManager};
use gasket_engine::bus::events::SessionKey;
use gasket_engine::config::{load_config, ModelRegistry};
use gasket_engine::providers::ProviderRegistry;
use gasket_engine::token_tracker::ModelPricing;

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
    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".gasket");

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
    let memory_store = Arc::new(MemoryStore::new().await);
    let sqlite_store = memory_store.sqlite_store().clone();

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

    let tools = super::registry::build_tool_registry(super::registry::ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_manager: None,
        extra_tools: vec![],
        sqlite_store: Some(sqlite_store),
        model_registry: Some(model_registry),
        provider_registry: Some(provider_registry),
    });

    // Create SubagentManager for spawn/spawn_parallel tools (uses dummy outbound channel)
    let (dummy_tx, _dummy_rx) = tokio::sync::mpsc::channel(16);
    let subagent_tools = Arc::new(super::registry::build_tool_registry(
        super::registry::ToolRegistryConfig {
            config: config.clone(),
            workspace: workspace.clone(),
            subagent_manager: None,
            extra_tools: vec![],
            sqlite_store: None,
            model_registry: None,
            provider_registry: None,
        },
    ));

    // Create model resolver for SubagentManager to support model_id switching in spawn tools
    let mut resolver_registry = ProviderRegistry::from_config(&config);
    if let Some(ref v) = vault {
        resolver_registry.with_vault(v.clone());
    }
    let model_resolver: Arc<dyn ModelResolver> = Arc::new(CliModelResolver {
        provider_registry: resolver_registry,
        model_registry: ModelRegistry::from_config(&config.agents),
    });

    let subagent_manager = Arc::new(
        SubagentManager::with_model_resolver(
            provider_info.provider.clone(),
            workspace.clone(),
            subagent_tools,
            dummy_tx,
            Some(model_resolver),
        )
        .await,
    );

    // Convert pricing info to ModelPricing
    let pricing = provider_info
        .pricing
        .map(|(input, output, currency)| ModelPricing::new(input, output, &currency));

    let agent = AgentLoop::with_pricing(
        provider_info.provider,
        workspace,
        agent_config,
        tools,
        memory_store,
        pricing,
    )
    .await
    .context("Failed to initialize agent (check workspace bootstrap files)")?
    .with_spawner(subagent_manager as Arc<dyn gasket_engine::SubagentSpawner>);

    let render_md = !opts.no_markdown;
    let use_streaming = !opts.no_stream;

    match opts.message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let session_key =
                SessionKey::new(gasket_engine::bus::events::ChannelType::Cli, "direct");
            if use_streaming {
                // Use channel-based streaming API
                let (mut event_rx, result_handle) = agent
                    .process_direct_streaming_with_channel(&msg, &session_key)
                    .await?;

                // Forward events to callback
                let forward_handle = tokio::spawn(async move {
                    while let Some(event) = event_rx.recv().await {
                        match event {
                            StreamEvent::Content(text) => print!("{}", text),
                            StreamEvent::Reasoning(text) => {
                                eprint!("{}", text.dimmed().italic());
                                std::io::stderr().flush().ok();
                            }
                            StreamEvent::TokenStats {
                                input_tokens,
                                output_tokens,
                                total_tokens,
                                cost,
                                currency,
                            } => {
                                let symbol = if currency == "CNY" { "¥" } else { "$" };
                                eprintln!(
                                    "\n[Token] Input: {} | Output: {} | Total: {} | Cost: {}{:.4}",
                                    input_tokens, output_tokens, total_tokens, symbol, cost
                                );
                            }
                            StreamEvent::Done => println!(),
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
                SessionKey::new(gasket_engine::bus::events::ChannelType::Cli, "interactive");

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
                                                StreamEvent::Content(text) => {
                                                    print!("{}", text);
                                                    std::io::stdout().flush().ok();
                                                }
                                                StreamEvent::Reasoning(text) => {
                                                    eprint!("{}", text.dimmed().italic());
                                                    std::io::stderr().flush().ok();
                                                }
                                                StreamEvent::TokenStats {
                                                    input_tokens,
                                                    output_tokens,
                                                    total_tokens,
                                                    cost,
                                                    currency,
                                                } => {
                                                    let symbol =
                                                        if currency == "CNY" { "¥" } else { "$" };
                                                    eprintln!("\n[Token] Input: {} | Output: {} | Total: {} | Cost: {}{:.4}",
                                                        input_tokens, output_tokens, total_tokens, symbol, cost);
                                                }
                                                StreamEvent::Done => {}
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
