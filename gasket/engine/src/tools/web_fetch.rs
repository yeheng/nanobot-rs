//! Web fetch tool for downloading web content

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::io::Cursor;
use tracing::{info, instrument, warn};
use url::Url;

use super::{build_client_with_proxy, simple_schema, Tool, ToolContext, ToolError, ToolResult};

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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
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
            .header("User-Agent", "Mozilla/5.0 (compatible; gasket/2.0)")
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

        let text = if content_type.contains("text/html") {
            let fallback: String = body.chars().take(2000).collect();
            extract_core_content(&args.url, body)
                .await
                .unwrap_or_else(|e| {
                    warn!("Core content extraction failed: {}", e);
                    fallback
                })
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

/// Extract core content from HTML using Readability algorithm.
///
/// Offloaded to `spawn_blocking` because DOM parsing is CPU-intensive
/// and must not block the async runtime.
async fn extract_core_content(url_str: &str, html: String) -> Result<String, anyhow::Error> {
    let url_clone = url_str.to_string();

    tokio::task::spawn_blocking(move || {
        let parsed_url =
            Url::parse(&url_clone).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;

        let mut cursor = Cursor::new(html.as_bytes());

        match readability::extractor::extract(&mut cursor, &parsed_url) {
            Ok(product) => {
                let title = product.title.trim();
                let text = product.text.trim();

                if text.len() < 100 {
                    Ok(fallback_extract(&html))
                } else {
                    Ok(format!("Title: {}\n\n{}", title, text))
                }
            }
            Err(_) => Ok(fallback_extract(&html)),
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("HTML extraction task panicked: {}", e))?
}

/// Brute-force fallback: strip scripts/styles/HTML tags via regex.
fn fallback_extract(html: &str) -> String {
    let re_script = Regex::new(r"(?is)<script.*?>.*?</script>").unwrap();
    let no_scripts = re_script.replace_all(html, " ");
    let re_style = Regex::new(r"(?is)<style.*?>.*?</style>").unwrap();
    let no_scripts = re_style.replace_all(&no_scripts, " ");

    let re_tags = Regex::new(r"(?is)<.*?>").unwrap();
    let raw_text = re_tags.replace_all(&no_scripts, " ");

    let re_ws = Regex::new(r"\s+").unwrap();
    re_ws.replace_all(&raw_text, " ").trim().to_string()
}
