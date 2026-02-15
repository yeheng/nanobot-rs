//! Web tools for searching and fetching content

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Web search tool using Brave Search API
pub struct WebSearchTool {
    client: Client,
    api_key: Option<String>,
}

impl WebSearchTool {
    /// Create a new web search tool
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using Brave Search API"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[("query", "string", true), ("count", "number", false)])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default = "default_count")]
            count: usize,
        }

        fn default_count() -> usize {
            5
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionError("Brave API key not configured".to_string()))?;

        debug!("Searching web for: {}", args.query);

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(&args.query),
            args.count
        );

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionError(format!(
                "API error: {} - {}",
                status, body
            )));
        }

        let search_response: BraveSearchResponse = response
            .json()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Parse error: {}", e)))?;

        // Format results
        let mut result = String::new();
        for (i, r) in search_response.web.results.iter().enumerate() {
            result.push_str(&format!(
                "{}. **{}**\n   {}\n   URL: {}\n\n",
                i + 1,
                r.title,
                r.description,
                r.url
            ));
        }

        if result.is_empty() {
            result = "No results found.".to_string();
        }

        Ok(result)
    }
}

/// Brave Search API response
#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: BraveWebResults,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    description: String,
    url: String,
}

/// Web fetch tool for downloading web content
pub struct WebFetchTool {
    client: Client,
    timeout_secs: u64,
    max_size: usize,
}

impl WebFetchTool {
    /// Create a new web fetch tool
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            timeout_secs: 120,
            max_size: 10_000_000, // 10 MB
        }
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
        simple_schema(&[("url", "string", true), ("prompt", "string", false)])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            url: String,
            #[serde(default)]
            prompt: Option<String>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        debug!("Fetching URL: {}", args.url);

        let response = self
            .client
            .get(&args.url)
            .header("User-Agent", "Mozilla/5.0 (compatible; nanobot/2.0)")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ToolError::ExecutionError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        // Get content type before consuming response
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Read error: {}", e)))?;

        // Simple text extraction for HTML
        let text = if content_type.contains("text/html") {
            // Basic HTML tag removal
            let text = body
                .replace("<script[^>]*>.*?</script>", "")
                .replace("<style[^>]*>.*?</style>", "")
                .replace("<[^>]+>", " ")
                .replace("&nbsp;", " ")
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">");

            // Collapse whitespace
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            body
        };

        // Truncate if too long
        let truncated = if text.len() > 8000 {
            format!(
                "{}...\n\n[Content truncated, {} chars total]",
                &text[..8000],
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

// URL encoding helper
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}
