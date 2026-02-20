//! Web tools for searching and fetching content

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Web search tool
pub struct WebSearchTool {
    client: Client,
    config: Option<crate::config::WebToolsConfig>,
}

impl WebSearchTool {
    /// Create a new web search tool
    pub fn new(config: Option<crate::config::WebToolsConfig>) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    async fn search_brave(&self, query: &str, count: usize) -> ToolResult {
        let api_key = self
            .config
            .as_ref()
            .and_then(|c| c.brave_api_key.as_ref())
            .ok_or_else(|| ToolError::ExecutionError("Brave API key not configured".to_string()))?;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(query),
            count
        );

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Brave API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionError(format!(
                "Brave Search API error (status {}): {}",
                status, body
            )));
        }

        let search_response: BraveSearchResponse = response.json().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to parse Brave API response: {}", e))
        })?;

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

    async fn search_tavily(&self, query: &str, count: usize) -> ToolResult {
        let api_key = self
            .config
            .as_ref()
            .and_then(|c| c.tavily_api_key.as_ref())
            .ok_or_else(|| {
                ToolError::ExecutionError("Tavily API key not configured".to_string())
            })?;

        let body = serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": count,
            "search_depth": "basic"
        });

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Tavily API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionError(format!(
                "Tavily API error (status {}): {}",
                status, body
            )));
        }

        let search_response: TavilySearchResponse = response.json().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to parse Tavily API response: {}", e))
        })?;

        let mut result = String::new();
        for (i, r) in search_response.results.iter().enumerate() {
            result.push_str(&format!(
                "{}. **{}**\n   {}\n   URL: {}\n\n",
                i + 1,
                r.title,
                r.content,
                r.url
            ));
        }

        if result.is_empty() {
            result = "No results found.".to_string();
        }

        Ok(result)
    }

    async fn search_exa(&self, query: &str, count: usize) -> ToolResult {
        let api_key = self
            .config
            .as_ref()
            .and_then(|c| c.exa_api_key.as_ref())
            .ok_or_else(|| ToolError::ExecutionError("Exa API key not configured".to_string()))?;

        let body = serde_json::json!({
            "query": query,
            "numResults": count,
            "contents": { "text": true }
        });

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Exa API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionError(format!(
                "Exa API error (status {}): {}",
                status, body
            )));
        }

        let search_response: ExaSearchResponse = response.json().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to parse Exa API response: {}", e))
        })?;

        let mut result = String::new();
        for (i, r) in search_response.results.iter().enumerate() {
            let title = r.title.as_deref().unwrap_or("No title");
            let text = r.text.as_deref().unwrap_or("No description");
            result.push_str(&format!(
                "{}. **{}**\n   {}\n   URL: {}\n\n",
                i + 1,
                title,
                text.chars().take(300).collect::<String>(),
                r.url
            ));
        }

        if result.is_empty() {
            result = "No results found.".to_string();
        }

        Ok(result)
    }

    async fn search_firecrawl(&self, query: &str, count: usize) -> ToolResult {
        let api_key = self
            .config
            .as_ref()
            .and_then(|c| c.firecrawl_api_key.as_ref())
            .ok_or_else(|| {
                ToolError::ExecutionError("Firecrawl API key not configured".to_string())
            })?;

        let body = serde_json::json!({
            "query": query,
            "limit": count
        });

        let response = self
            .client
            .post("https://api.firecrawl.dev/v1/search")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                ToolError::ExecutionError(format!("Firecrawl API request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionError(format!(
                "Firecrawl API error (status {}): {}",
                status, body
            )));
        }

        let search_response: FirecrawlSearchResponse = response.json().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to parse Firecrawl API response: {}", e))
        })?;

        let mut result = String::new();
        for (i, r) in search_response.data.iter().enumerate() {
            let title = r.title.as_deref().unwrap_or("No title");
            let desc = r.description.as_deref().unwrap_or("No description");
            result.push_str(&format!(
                "{}. **{}**\n   {}\n   URL: {}\n\n",
                i + 1,
                title,
                desc,
                r.url
            ));
        }

        if result.is_empty() {
            result = "No results found.".to_string();
        }

        Ok(result)
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using the configured provider (Brave, Tavily, Exa, Firecrawl)"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("query", "string", true, "Search query string"),
            (
                "count",
                "number",
                false,
                "Number of results to return (default 5)",
            ),
        ])
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

        let provider = self
            .config
            .as_ref()
            .and_then(|c| c.search_provider.as_deref())
            .unwrap_or("brave")
            .to_lowercase();

        debug!(
            "Searching web with provider '{}' for: {}",
            provider, args.query
        );

        match provider.as_str() {
            "tavily" => self.search_tavily(&args.query, args.count).await,
            "exa" => self.search_exa(&args.query, args.count).await,
            "firecrawl" => self.search_firecrawl(&args.query, args.count).await,
            _ => self.search_brave(&args.query, args.count).await,
        }
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

#[derive(Debug, Deserialize)]
struct TavilySearchResponse {
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    content: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ExaSearchResponse {
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    title: Option<String>,
    text: Option<String>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct FirecrawlSearchResponse {
    data: Vec<FirecrawlResult>,
}

#[derive(Debug, Deserialize)]
struct FirecrawlResult {
    title: Option<String>,
    description: Option<String>,
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

/// Strip HTML tags and decode common entities.
///
/// Removes `<script>` and `<style>` blocks entirely, strips all other tags,
/// decodes common HTML entities, and collapses whitespace.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_script = false;
    let mut in_style = false;

    while i < len {
        // Check for opening <script or <style tags
        if chars[i] == '<' {
            // Try to match <script or </script>
            let rest: String = chars[i..std::cmp::min(i + 10, len)].iter().collect();
            let rest_lower = rest.to_lowercase();

            if rest_lower.starts_with("<script") {
                in_script = true;
                // Skip to closing >
                while i < len && chars[i] != '>' {
                    i += 1;
                }
                i += 1; // skip '>'
                continue;
            }

            if rest_lower.starts_with("</script") {
                in_script = false;
                while i < len && chars[i] != '>' {
                    i += 1;
                }
                i += 1;
                continue;
            }

            if rest_lower.starts_with("<style") {
                in_style = true;
                while i < len && chars[i] != '>' {
                    i += 1;
                }
                i += 1;
                continue;
            }

            if rest_lower.starts_with("</style") {
                in_style = false;
                while i < len && chars[i] != '>' {
                    i += 1;
                }
                i += 1;
                continue;
            }

            // Skip any other tag
            if !in_script && !in_style {
                while i < len && chars[i] != '>' {
                    i += 1;
                }
                i += 1;
                result.push(' ');
                continue;
            }
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        // Decode HTML entities
        if chars[i] == '&' {
            let entity_end = chars[i..std::cmp::min(i + 10, len)]
                .iter()
                .position(|&c| c == ';');
            if let Some(end) = entity_end {
                let entity: String = chars[i..i + end + 1].iter().collect();
                match entity.as_str() {
                    "&nbsp;" => result.push(' '),
                    "&amp;" => result.push('&'),
                    "&lt;" => result.push('<'),
                    "&gt;" => result.push('>'),
                    "&quot;" => result.push('"'),
                    "&#39;" | "&apos;" => result.push('\''),
                    _ => result.push_str(&entity),
                }
                i += end + 1;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    // Collapse whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

// URL encoding helper
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}
