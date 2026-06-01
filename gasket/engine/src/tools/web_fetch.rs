//! Web fetch tool for downloading web content

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument, warn};

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

/// Extract core content from HTML.
///
/// 1. Primary: `dom_smoothie` — Mozilla Readability algorithm, finds the main
///    article content and strips navigation/ads/sidebars.
/// 2. Fallback: `fast_html2md` — streaming HTML→Markdown rewriter (lol_html),
///    extremely fast and memory-efficient, preserves headings/lists/links.
///
/// Offloaded to `spawn_blocking` because DOM parsing is CPU-intensive.
async fn extract_core_content(url_str: &str, html: String) -> Result<String, anyhow::Error> {
    let url_clone = url_str.to_string();

    tokio::task::spawn_blocking(move || {
        // ── Primary: dom_smoothie ───────────────────────────────────────
        let dom_result = (|| -> anyhow::Result<String> {
            let cfg = dom_smoothie::Config::default();
            let mut readability =
                dom_smoothie::Readability::new(html.clone(), Some(url_clone.as_str()), Some(cfg))
                    .map_err(|e| anyhow::anyhow!("Readability init failed: {}", e))?;
            let article = readability
                .parse()
                .map_err(|e| anyhow::anyhow!("Readability parse failed: {}", e))?;
            let text = article.text_content.trim();
            if text.len() < 100 {
                return Err(anyhow::anyhow!(
                    "Extracted text too short ({} chars)",
                    text.len()
                ));
            }
            let title = article.title.trim();
            Ok(format!("Title: {}\n\n{}", title, text))
        })();

        match dom_result {
            Ok(text) => return Ok(text),
            Err(e) => {
                tracing::warn!("dom_smoothie extraction failed: {}", e);
            }
        }

        // ── Fallback: fast_html2md ──────────────────────────────────────
        let md = html2md::rewrite_html(&html, false);
        let md = md.trim();
        if md.len() >= 50 {
            Ok(md.to_string())
        } else {
            Err(anyhow::anyhow!(
                "Both extractors failed (dom_smoothie + fast_html2md)"
            ))
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("HTML extraction task panicked: {}", e))?
}
