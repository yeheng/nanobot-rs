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
pub async fn start_mcp_servers(
    config: &crate::config::ToolsConfig,
) -> (Arc<McpManager>, Vec<Box<dyn Tool>>) {
    let mut manager = McpManager::new();

    // New grouped format: stdio servers
    for (name, cfg) in &config.mcp.stdio {
        let transport = types::McpTransport::Stdio {
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
        };
        manager.add_server(name.clone(), McpServerConfig { transport });
    }

    // New grouped format: remote servers
    for (name, cfg) in &config.mcp.remote {
        let transport = match cfg {
            crate::config::RemoteMcpConfig::Simple { url } => types::McpTransport::Http {
                url: url.clone(),
                auth: types::McpAuth::default(),
                timeout: 30,
            },
            crate::config::RemoteMcpConfig::Enhanced {
                transport,
                auth,
                timeout,
                ..
            } => {
                let mcp_auth = types::McpAuth {
                    api_key: auth.api_key.clone(),
                    bearer_token: auth.bearer_token.clone(),
                    headers: auth.headers.clone(),
                };
                match transport {
                    crate::config::RemoteTransportConfig::Http { url } => {
                        types::McpTransport::Http {
                            url: url.clone(),
                            auth: mcp_auth,
                            timeout: *timeout,
                        }
                    }
                    crate::config::RemoteTransportConfig::Sse { url } => types::McpTransport::Sse {
                        url: url.clone(),
                        auth: mcp_auth,
                        timeout: *timeout,
                    },
                    crate::config::RemoteTransportConfig::WebSocket { url } => {
                        types::McpTransport::WebSocket {
                            url: url.clone(),
                            auth: mcp_auth,
                            timeout: *timeout,
                        }
                    }
                }
            }
        };
        manager.add_server(name.clone(), McpServerConfig { transport });
    }

    // Legacy flat format (backward compatibility)
    for (name, cfg) in &config.mcp_servers {
        let transport = if let Some(url) = &cfg.url {
            types::McpTransport::Http {
                url: url.clone(),
                auth: types::McpAuth::default(),
                timeout: 30,
            }
        } else if let Some(command) = &cfg.command {
            types::McpTransport::Stdio {
                command: command.clone(),
                args: cfg.args.clone().unwrap_or_default(),
                env: None,
            }
        } else {
            warn!("MCP server '{}' has no command or url, skipping", name);
            continue;
        };
        manager.add_server(name.clone(), McpServerConfig { transport });
    }

    if let Err(e) = manager.start_all().await {
        warn!("Error starting MCP servers: {}", e);
    }

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

    info!("MCP bridge: {} tools ready", tools.len());
    (manager, tools)
}
