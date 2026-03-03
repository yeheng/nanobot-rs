//! Agent 命令实现

use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use tracing::{info, Level};

use nanobot_core::agent::{AgentConfig, AgentLoop, AgentResponse, StreamCallback, StreamEvent};
use nanobot_core::bus::events::SessionKey;
use nanobot_core::config::{load_config, Config};
use nanobot_core::tools::{
    EditFileTool, ExecTool, ListDirTool, ReadFileTool, SpawnTool, ToolMetadata, ToolRegistry,
    WebFetchTool, WebSearchTool, WriteFileTool,
};

use crate::cli::AgentOptions;

/// Resolve the exec workspace directory from config or default to $HOME/.nanobot.
///
/// Creates the directory if it doesn't exist.
pub fn resolve_exec_workspace(config: &Config, fallback: &std::path::Path) -> std::path::PathBuf {
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
        info!("Created exec workspace: {:?}", workspace_path);
    }

    workspace_path
}

/// Build AgentConfig from the config file, applying defaults for zero-valued fields.
pub fn build_agent_config(config: &Config) -> AgentConfig {
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
    let mut agent_config = build_agent_config(&config);
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
    let mcp_tools = if !config.tools.mcp_servers.is_empty() {
        println!("Starting MCP servers...");
        let (_mcp_manager, tools) =
            nanobot_core::mcp::start_mcp_servers(&config.tools.mcp_servers).await;
        println!("  {} MCP tools loaded", tools.len());
        tools
    } else {
        Vec::new()
    };

    // Build tool registry (CLI mode: no bus/cron, but support web tools)
    let tools = build_tool_registry(&config, &workspace, mcp_tools, None);

    let agent = AgentLoop::new(provider_info.provider, workspace, agent_config, tools)
        .await
        .context("Failed to initialize agent (check workspace bootstrap files)")?;
    let render_md = !opts.no_markdown;
    let use_streaming = !opts.no_stream;

    // Create streaming callback for progressive CLI output
    // Note: Use stdout for content (piping), stderr for status/logs
    let stream_callback: StreamCallback = Box::new(|event| {
        match event {
            StreamEvent::Content(_text) => {}
            StreamEvent::Reasoning(text) => {
                eprint!("{}", text.dimmed().italic());
                std::io::stderr().flush().ok();
            }
            StreamEvent::ToolStart { name: _ } => {}
            StreamEvent::ToolEnd { name: _, output: _ } => {}
            StreamEvent::Done => {
                // Ensure stdout ends with newline for clean separation
                eprintln!("\n");
            }
        }
    });

    match opts.message {
        Some(msg) => {
            // Single message mode
            info!("Processing message: {}", msg);
            let session_key = SessionKey::new(nanobot_core::bus::events::ChannelType::Cli, "direct");
            if use_streaming {
                let _response = agent
                    .process_direct_with_callback(&msg, &session_key, Some(&stream_callback))
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

            let interactive_session = SessionKey::new(nanobot_core::bus::events::ChannelType::Cli, "interactive");

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
                            match agent
                                .process_direct_with_callback(
                                    line,
                                    &interactive_session,
                                    Some(&stream_callback),
                                )
                                .await
                            {
                                Ok(_response) => {
                                    println!();
                                }
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

/// Build tool registry for CLI agent mode
fn build_tool_registry(
    config: &Config,
    workspace: &std::path::Path,
    mcp_tools: Vec<Box<dyn nanobot_core::tools::Tool>>,
    subagent_manager: Option<Arc<nanobot_core::agent::SubagentManager>>,
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

    // MCP tools (metadata assigned by MCP manager)
    for mcp_tool in mcp_tools {
        tools.register(mcp_tool);
    }

    tools
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
