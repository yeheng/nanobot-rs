//! MCP (Model Context Protocol) client implementation
//!
//! Implements JSON-RPC 2.0 over stdio for communicating with MCP servers.

pub mod client;
pub mod manager;
pub mod tool;
pub mod types;

pub use client::McpClient;
pub use manager::McpManager;
pub use tool::McpToolBridge;
pub use types::{McpServerConfig, McpTool};

use crate::tools::Tool;
use std::sync::Arc;
use tracing::{info, warn};

/// Start all configured MCP servers and return bridge tools for registration.
///
/// This function:
/// 1. Parses MCP server configs from the config schema
/// 2. Starts all servers via `McpManager`
/// 3. Returns a list of `Box<dyn Tool>` adapters ready for `ToolRegistry::register()`
pub async fn start_mcp_servers(
    configs: &std::collections::HashMap<String, crate::config::McpServerConfig>,
) -> (Arc<McpManager>, Vec<Box<dyn Tool>>) {
    let mut manager = McpManager::new();

    for (name, cfg) in configs {
        if let Some(command) = &cfg.command {
            let mcp_cfg = McpServerConfig {
                command: command.clone(),
                args: cfg.args.clone().unwrap_or_default(),
                env: None,
            };
            manager.add_server(name.clone(), mcp_cfg);
        } else {
            warn!("MCP server '{}' has no command configured, skipping", name);
        }
    }

    if let Err(e) = manager.start_all().await {
        warn!("Error starting MCP servers: {}", e);
    }

    // Collect tool metadata before wrapping manager
    let tool_info: Vec<(String, McpTool)> = manager.get_all_tools().await;

    let manager = Arc::new(manager);

    let tools: Vec<Box<dyn Tool>> = tool_info
        .iter()
        .map(|(server, mcp_tool)| {
            Box::new(McpToolBridge::new(
                server.clone(),
                mcp_tool,
                manager.clone(),
            )) as Box<dyn Tool>
        })
        .collect();

    info!(
        "MCP bridge: {} tools ready from {} servers",
        tools.len(),
        configs.len()
    );

    (manager, tools)
}
