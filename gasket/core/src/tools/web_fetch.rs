//! Web fetch tool for downloading web content

use async_trait::async_trait;
use reqwest::{Client, Proxy};
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument, warn};

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Build a reqwest client with proxy configuration.
///
/// Priority order for proxy configuration:
/// 1. Explicit proxy URLs in config (http_proxy, https_proxy, socks5_proxy)
/// 2. System environment variables (if use_env_proxy is true)
fn build_client_with_proxy(
    config: Option<&crate::config::WebToolsConfig>,
) -> Result<Client, ToolError> {
    let mut builder = Client::builder();

    if let Some(cfg) = config {
        // Check for explicit proxy configuration
        let has_explicit_proxy =
            cfg.http_proxy.is_some() || cfg.https_proxy.is_some() || cfg.socks5_proxy.is_some();

        if has_explicit_proxy {
            // Add HTTP proxy for HTTP requests
            if let Some(ref proxy_url) = cfg.http_proxy {
                match Proxy::http(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid HTTP proxy URL '{}': {}", proxy_url, e),
                }
            }

            // Add HTTPS proxy for HTTPS requests
            if let Some(ref proxy_url) = cfg.https_proxy {
                match Proxy::https(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid HTTPS proxy URL '{}': {}", proxy_url, e),
                }
            }

            // Add SOCKS5 proxy (applies to all requests)
            if let Some(ref proxy_url) = cfg.socks5_proxy {
                match Proxy::all(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid SOCKS5 proxy URL '{}': {}", proxy_url, e),
                }
            }
        } else if cfg.use_env_proxy {
            // Use system proxy from environment variables (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY)
            // This is the default behavior when no explicit proxy is configured
            builder = builder.use_rustls_tls();
        }
    }

    builder
        .build()
        .map_err(|e| ToolError::ExecutionError(format!("Failed to create HTTP client: {}", e)))
}

/// Web fetch tool for downloading web content
pub struct WebFetchTool {
    client: Client,
    timeout_secs: u64,
    max_size: usize,
}

impl WebFetchTool {
    /// Create a new web fetch tool with default settings
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            timeout_secs: 120,
            max_size: 10_000_000, // 10 MB
        }
    }

    /// Create a new web fetch tool with proxy configuration
    pub fn with_config(config: Option<crate::config::WebToolsConfig>) -> Result<Self, ToolError> {
        let client = build_client_with_proxy(config.as_ref())?;
        Ok(Self {
            client,
            timeout_secs: 120,
            max_size: 10_000_000, // 10 MB
        })
    }

    /// Set timeout in seconds
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Set max response size in bytes
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch and extract text content from a web page"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("url", "string", true, "URL of the web page to fetch"),
            (
                "prompt",
                "string",
                false,
                "Optional prompt describing what to extract from the page",
            ),
        ])
    }

    #[instrument(name = "tool.web_fetch", skip_all)]
    async fn execute(&self, args: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            url: String,
            #[serde(default)]
            prompt: Option<String>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        info!("Fetching URL: {}", args.url);

        let response = self
            .client
            .get(&args.url)
            .header("User-Agent", "Mozilla/5.0 (compatible; nanobot/2.0)")
            .send()
            .await
            .map_err(|e| {
                ToolError::ExecutionError(format!("Failed to fetch URL '{}': {}", args.url, e))
            })?;

        if !response.status().is_success() {
            return Err(ToolError::ExecutionError(format!(
                "HTTP error {} when fetching '{}'",
                response.status(),
                args.url
            )));
        }

        // Get content type before consuming response
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = response.text().await.map_err(|e| {
            ToolError::ExecutionError(format!(
                "Failed to read response body from '{}': {}",
                args.url, e
            ))
        })?;

        // Simple text extraction for HTML
        let text = if content_type.contains("text/html") {
            strip_html(&body)
        } else {
            body
        };

        // Truncate if too long (UTF-8 safe)
        let truncated = if text.len() > 8000 {
            let safe_len = text
                .char_indices()
                .nth(8000)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!(
                "{}...\n\n[Content truncated, {} chars total]",
                &text[..safe_len],
                text.len()
            )
        } else if let Some(prompt) = &args.prompt {
            format!("Prompt: {}\n\nContent:\n{}", prompt, text)
        } else {
            text
        };

        Ok(truncated)
    }
}

/// Strip HTML tags and convert to plain text.
///
/// Uses the well-tested `html2text` crate for robust HTML parsing,
/// handling edge cases like malformed tags, entities, and nested structures.
fn strip_html(html: &str) -> String {
    match html2text::from_read(html.as_bytes(), 10000) {
        Ok(text) => text
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Err(_) => {
            // html2text failed — return a safely truncated raw snippet
            // instead of attempting fragile hand-rolled tag stripping.
            let truncated: String = html.chars().take(2000).collect();
            if truncated.len() < html.len() {
                format!(
                    "[HTML parsing failed. Showing raw snippet:]\n{}...",
                    truncated
                )
            } else {
                format!("[HTML parsing failed. Showing raw snippet:]\n{}", truncated)
            }
        }
    }
}
