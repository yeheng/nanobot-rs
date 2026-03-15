use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

/// Authentication configuration for MCP transports
#[derive(Debug, Clone, Default)]
pub struct McpAuth {
    /// API key sent as X-API-Key header
    pub api_key: Option<String>,
    /// Bearer token sent as Authorization: Bearer header
    pub bearer_token: Option<String>,
    /// Custom headers to include in requests
    pub headers: Option<HashMap<String, String>>,
}

/// MCP transport type
#[derive(Debug, Clone)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    },
    Http {
        url: String,
        auth: McpAuth,
        timeout: u64,
    },
    Sse {
        url: String,
        auth: McpAuth,
        timeout: u64,
    },
    WebSocket {
        url: String,
        auth: McpAuth,
        timeout: u64,
    },
}

/// MCP server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub transport: McpTransport,
}
