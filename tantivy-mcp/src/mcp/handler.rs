//! MCP request handler.

use serde_json::json;
use tracing::{debug, info};

use super::tools::ToolRegistry;
use super::transport::StdioTransport;
use super::types::*;
use crate::Result;

/// MCP protocol handler.
pub struct McpHandler {
    transport: StdioTransport,
    tools: ToolRegistry,
    initialized: bool,
}

impl McpHandler {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            transport: StdioTransport::new(),
            tools,
            initialized: false,
        }
    }

    /// Run the MCP server loop.
    pub fn run(&mut self) -> Result<()> {
        info!("Starting MCP server");

        while let Some(request) = self.transport.read_request() {
            debug!("Processing request: {:?}", request.method);

            let response = match request.method.as_str() {
                "initialize" => self.handle_initialize(&request),
                "notifications/initialized" => {
                    self.initialized = true;
                    continue; // No response for notifications
                }
                "tools/list" => self.handle_tools_list(&request),
                "tools/call" => self.handle_tools_call(&request),
                method => {
                    let error = JsonRpcError::method_not_found(method);
                    JsonRpcResponse::error(request.id, error)
                }
            };

            self.transport.write_response(&response)?;
        }

        info!("MCP server shutting down");
        Ok(())
    }

    fn handle_initialize(&mut self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {}),
            },
            server_info: ServerInfo {
                name: "tantivy-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        JsonRpcResponse::success(request.id.clone(), serde_json::to_value(result).unwrap())
    }

    fn handle_tools_list(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let tools = self.tools.list_tools();
        JsonRpcResponse::success(request.id.clone(), json!({ "tools": tools }))
    }

    fn handle_tools_call(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let params = match &request.params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    JsonRpcError::invalid_params("Missing params"),
                );
            }
        };

        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(name) => name,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    JsonRpcError::invalid_params("Missing tool name"),
                );
            }
        };

        let arguments = params.get("arguments").cloned();

        info!("Calling tool: {}", tool_name);

        match self.tools.call_tool(tool_name, arguments) {
            Ok(result) => {
                JsonRpcResponse::success(request.id.clone(), serde_json::to_value(result).unwrap())
            }
            Err(e) => {
                let error_result = ToolResult::error(e.to_string());
                JsonRpcResponse::success(
                    request.id.clone(),
                    serde_json::to_value(error_result).unwrap(),
                )
            }
        }
    }
}
