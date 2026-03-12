//! Stdio transport for MCP.

use std::io;
use tracing::debug;

use super::types::{JsonRpcRequest, JsonRpcResponse};

/// Stdio transport for MCP communication.
pub struct StdioTransport {
    stdin: tokio::io::BufReader<tokio::io::Stdin>,
    stdout: tokio::io::Stdout,
}

impl StdioTransport {
    pub fn new() -> Self {
        Self {
            stdin: tokio::io::BufReader::new(tokio::io::stdin()),
            stdout: tokio::io::stdout(),
        }
    }

    /// Read a single JSON-RPC request from stdin.
    pub async fn read_request(&mut self) -> Option<JsonRpcRequest> {
        loop {
            let mut line = String::new();
            match tokio::io::AsyncBufReadExt::read_line(&mut self.stdin, &mut line).await {
                Ok(0) => {
                    // EOF
                    debug!("Received EOF on stdin");
                    return None;
                }
                Ok(_) => {
                    let line = line.trim();
                    if line.is_empty() {
                        // Skip empty lines and continue reading
                        continue;
                    }
                    debug!("Received: {}", &line[..line.len().min(200)]);
                    match serde_json::from_str::<JsonRpcRequest>(line) {
                        Ok(request) => return Some(request),
                        Err(e) => {
                            debug!("Failed to parse request: {}", e);
                            // Continue reading on parse error instead of returning None
                            continue;
                        }
                    }
                }
                Err(e) => {
                    debug!("Error reading from stdin: {}", e);
                    return None;
                }
            }
        }
    }

    /// Write a JSON-RPC response to stdout.
    pub async fn write_response(&mut self, response: &JsonRpcResponse) -> io::Result<()> {
        let json = serde_json::to_string(response)?;
        debug!("Sending: {}", &json[..json.len().min(200)]);
        use tokio::io::AsyncWriteExt;
        self.stdout
            .write_all(format!("{}\n", json).as_bytes())
            .await?;
        self.stdout.flush().await?;
        Ok(())
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}
