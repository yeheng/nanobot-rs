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
    },
}

/// MCP server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub transport: McpTransport,
}
