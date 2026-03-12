//! Agent 命令实现

use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};

use nanobot_core::agent::memory::MemoryStore;
use nanobot_core::agent::{AgentLoop, AgentResponse, StreamEvent};
use nanobot_core::bus::events::SessionKey;
use nanobot_core::config::{load_config, ModelRegistry};
use nanobot_core::providers::ProviderRegistry;

use crate::cli::AgentOptions;

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
        .join(".nanobot");

    // Find a provider
    let provider_info = crate::provider::find_provider(&config)?;

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

    // Start MCP servers (if configured)
    let mcp_tools = if !config.tools.mcp.stdio.is_empty()
        || !config.tools.mcp.remote.is_empty()
        || !config.tools.mcp_servers.is_empty()
    {
        println!("Starting MCP servers...");
        let (_mcp_manager, tools) = nanobot_core::mcp::start_mcp_servers(&config.tools).await;
        println!("  {} MCP tools loaded", tools.len());
        tools
    } else {
        Vec::new()
    };

    // Build tool registry (CLI mode: no bus/cron)
    let memory_store = Arc::new(MemoryStore::new().await);
    let sqlite_store = memory_store.sqlite_store().clone();

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

    let tools = super::registry::build_tool_registry(super::registry::ToolRegistryConfig {
        config,
        workspace: workspace.clone(),
        mcp_tools,
        subagent_manager: None,
        extra_tools: vec![],
        sqlite_store: Some(sqlite_store),
        model_registry: Some(model_registry),
        provider_registry: Some(provider_registry),
    });

    let mut agent = AgentLoop::with_memory_store(
        provider_info.provider,
        workspace,
        agent_config,
        tools,
        memory_store,
    )
    .await
    .context("Failed to initialize agent (check workspace bootstrap files)")?;

    // Set pricing configuration if available
    if let Some((input_price, output_price, currency)) = provider_info.pricing {
        agent.set_pricing(input_price, output_price, &currency);
    }

    let render_md = !opts.no_markdown;
    let use_streaming = !opts.no_stream;

    match opts.message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let session_key =
                SessionKey::new(nanobot_core::bus::events::ChannelType::Cli, "direct");
            if use_streaming {
                agent
                    .process_direct_streaming(&msg, &session_key, |event| match event {
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
                    })
                    .await?;
            } else {
                let response = agent.process_direct(&msg, &session_key).await?;
                print_response_with_reasoning(&response, render_md);
            }
        }
        None => {
            // Interactive mode
            println!("🐈 nanobot interactive mode. Type '/help' for commands, '/exit' to quit.\n");

            let mut line_editor = Reedline::create();
            let prompt =
                DefaultPrompt::new(DefaultPromptSegment::Empty, DefaultPromptSegment::Empty);

            let interactive_session =
                SessionKey::new(nanobot_core::bus::events::ChannelType::Cli, "interactive");

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
                                "🐈 nanobot commands:\n\
                                 /new  — Start a new conversation\n\
                                 /help — Show available commands\n\
                                 /exit — Exit the REPL"
                            );
                            continue;
                        }

                        // Process the message
                        if use_streaming {
                            println!();
                            match agent.process_direct_streaming(line, &interactive_session, |event| {
                                match event {
                                    StreamEvent::Content(text) => {
                                        print!("{}", text);
                                        std::io::stdout().flush().ok();
                                    }
                                    StreamEvent::Reasoning(text) => {
                                        eprint!("{}", text.dimmed().italic());
                                        std::io::stderr().flush().ok();
                                    }
                                    StreamEvent::TokenStats { input_tokens, output_tokens, total_tokens, cost, currency } => {
                                        let symbol = if currency == "CNY" { "¥" } else { "$" };
                                        eprintln!("\n[Token] Input: {} | Output: {} | Total: {} | Cost: {}{:.4}",
                                            input_tokens, output_tokens, total_tokens, symbol, cost);
                                    }
                                    StreamEvent::Done => {}
                                    _ => {}
                                }
                            }).await {
                                Ok(_) => println!("\n"),
                                Err(e) => println!("\n{} {}\n", "Error:".red(), e),
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
