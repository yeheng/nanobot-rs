//! Web search tool with pluggable search provider backends.

use async_trait::async_trait;
use reqwest::Client;
use reqwest::Proxy;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument, warn};

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Build a reqwest client with proxy configuration.
///
/// Priority order:
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
            // Use system proxy from environment variables
            builder = builder.use_rustls_tls();
        }
    } else {
        // No config - use default client (which respects env vars by default)
    }

    builder
        .build()
        .map_err(|e| ToolError::ExecutionError(format!("Failed to create HTTP client: {}", e)))
}

// ── Search result abstraction ───────────────────────────────

/// A single search result from any provider.
struct SearchHit {
    title: String,
    snippet: String,
    url: String,
}

/// Trait for pluggable search backends.
#[async_trait]
trait SearchProvider: Send + Sync {
    /// Execute a search and return normalized results.
    async fn search(
        &self,
        client: &Client,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchHit>, ToolError>;
}

/// Format a list of search hits into a human-readable string.
fn format_hits(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "No results found.".to_string();
    }
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}**\n   {}\n   URL: {}\n\n",
            i + 1,
            h.title,
            h.snippet,
            h.url
        ));
    }
    out
}

// ── Provider implementations ────────────────────────────────

// -- Brave --

struct BraveProvider<'a> {
    api_key: &'a str,
}

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

#[async_trait]
impl SearchProvider for BraveProvider<'_> {
    async fn search(
        &self,
        client: &Client,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchHit>, ToolError> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(query),
            count
        );

        let resp: BraveSearchResponse =
            send_get(client, &url, "X-Subscription-Token", self.api_key, "Brave").await?;

        Ok(resp
            .web
            .results
            .into_iter()
            .map(|r| SearchHit {
                title: r.title,
                snippet: r.description,
                url: r.url,
            })
            .collect())
    }
}

// -- Tavily --

struct TavilyProvider<'a> {
    api_key: &'a str,
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

#[async_trait]
impl SearchProvider for TavilyProvider<'_> {
    async fn search(
        &self,
        client: &Client,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchHit>, ToolError> {
        let body = serde_json::json!({
            "api_key": self.api_key,
            "query": query,
            "max_results": count,
            "search_depth": "basic"
        });

        let resp: TavilySearchResponse = send_post_json(
            client,
            "https://api.tavily.com/search",
            &body,
            None,
            "Tavily",
        )
        .await?;

        Ok(resp
            .results
            .into_iter()
            .map(|r| SearchHit {
                title: r.title,
                snippet: r.content,
                url: r.url,
            })
            .collect())
    }
}

// -- Exa --

struct ExaProvider<'a> {
    api_key: &'a str,
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

#[async_trait]
impl SearchProvider for ExaProvider<'_> {
    async fn search(
        &self,
        client: &Client,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchHit>, ToolError> {
        let body = serde_json::json!({
            "query": query,
            "numResults": count,
            "contents": { "text": true }
        });

        let resp: ExaSearchResponse = send_post_json(
            client,
            "https://api.exa.ai/search",
            &body,
            Some(("x-api-key", self.api_key)),
            "Exa",
        )
        .await?;

        Ok(resp
            .results
            .into_iter()
            .map(|r| SearchHit {
                title: r.title.unwrap_or_else(|| "No title".to_string()),
                snippet: r
                    .text
                    .map(|t| t.chars().take(300).collect())
                    .unwrap_or_else(|| "No description".to_string()),
                url: r.url,
            })
            .collect())
    }
}

// -- Firecrawl --

struct FirecrawlProvider<'a> {
    api_key: &'a str,
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

#[async_trait]
impl SearchProvider for FirecrawlProvider<'_> {
    async fn search(
        &self,
        client: &Client,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchHit>, ToolError> {
        let body = serde_json::json!({
            "query": query,
            "limit": count
        });

        let resp: FirecrawlSearchResponse = send_post_json(
            client,
            "https://api.firecrawl.dev/v1/search",
            &body,
            Some(("Authorization", &format!("Bearer {}", self.api_key))),
            "Firecrawl",
        )
        .await?;

        Ok(resp
            .data
            .into_iter()
            .map(|r| SearchHit {
                title: r.title.unwrap_or_else(|| "No title".to_string()),
                snippet: r
                    .description
                    .unwrap_or_else(|| "No description".to_string()),
                url: r.url,
            })
            .collect())
    }
}

// ── Shared HTTP helpers ─────────────────────────────────────

/// Send a GET request with a header-based API key, deserialize the JSON body.
async fn send_get<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    key_header: &str,
    api_key: &str,
    provider_name: &str,
) -> Result<T, ToolError> {
    let response = client
        .get(url)
        .header(key_header, api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            ToolError::ExecutionError(format!("{} API request failed: {}", provider_name, e))
        })?;

    check_status(&response, provider_name).await?;

    response.json::<T>().await.map_err(|e| {
        ToolError::ExecutionError(format!(
            "Failed to parse {} API response: {}",
            provider_name, e
        ))
    })
}

/// Send a POST request with a JSON body, deserialize the JSON response.
///
/// `auth_header` is an optional `(header_name, header_value)` tuple.
async fn send_post_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    body: &Value,
    auth_header: Option<(&str, &str)>,
    provider_name: &str,
) -> Result<T, ToolError> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(body);

    if let Some((key, value)) = auth_header {
        req = req.header(key, value);
    }

    let response = req.send().await.map_err(|e| {
        ToolError::ExecutionError(format!("{} API request failed: {}", provider_name, e))
    })?;

    check_status(&response, provider_name).await?;

    response.json::<T>().await.map_err(|e| {
        ToolError::ExecutionError(format!(
            "Failed to parse {} API response: {}",
            provider_name, e
        ))
    })
}

/// Check HTTP status, returning a `ToolError` on non-2xx responses.
async fn check_status(response: &reqwest::Response, provider_name: &str) -> Result<(), ToolError> {
    if !response.status().is_success() {
        let status = response.status();
        // We can't consume the body here because response is borrowed,
        // so we just report the status code.
        return Err(ToolError::ExecutionError(format!(
            "{} API error (status {})",
            provider_name, status
        )));
    }
    Ok(())
}

// ── WebSearchTool (public API) ──────────────────────────────

/// Web search tool
pub struct WebSearchTool {
    client: Client,
    config: Option<crate::config::WebToolsConfig>,
}

impl WebSearchTool {
    /// Create a new web search tool with optional proxy configuration
    pub fn new(config: Option<crate::config::WebToolsConfig>) -> Self {
        let client = build_client_with_proxy(config.as_ref()).unwrap_or_else(|e| {
            warn!(
                "Failed to create HTTP client with proxy config: {}. Using default client.",
                e
            );
            Client::new()
        });
        Self { client, config }
    }

    /// Resolve the configured provider and execute the search.
    async fn do_search(&self, query: &str, count: usize) -> ToolResult {
        let provider_name = self
            .config
            .as_ref()
            .and_then(|c| c.search_provider.as_deref())
            .unwrap_or("brave")
            .to_lowercase();

        info!(
            "[WebSearch] Using '{}' API to search for: {}",
            provider_name, query
        );

        let hits = match provider_name.as_str() {
            "tavily" => {
                let key = self.require_key(|c| c.tavily_api_key.as_ref(), "Tavily")?;
                TavilyProvider { api_key: &key }
                    .search(&self.client, query, count)
                    .await?
            }
            "exa" => {
                let key = self.require_key(|c| c.exa_api_key.as_ref(), "Exa")?;
                ExaProvider { api_key: &key }
                    .search(&self.client, query, count)
                    .await?
            }
            "firecrawl" => {
                let key = self.require_key(|c| c.firecrawl_api_key.as_ref(), "Firecrawl")?;
                FirecrawlProvider { api_key: &key }
                    .search(&self.client, query, count)
                    .await?
            }
            _ => {
                let key = self.require_key(|c| c.brave_api_key.as_ref(), "Brave")?;
                BraveProvider { api_key: &key }
                    .search(&self.client, query, count)
                    .await?
            }
        };

        Ok(format_hits(&hits))
    }

    /// Extract an API key from config, or return a descriptive error.
    fn require_key<F>(&self, extractor: F, name: &str) -> Result<String, ToolError>
    where
        F: FnOnce(&crate::config::WebToolsConfig) -> Option<&String>,
    {
        self.config
            .as_ref()
            .and_then(extractor)
            .cloned()
            .ok_or_else(|| ToolError::ExecutionError(format!("{} API key not configured", name)))
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

    #[instrument(name = "tool.web_search", skip_all)]
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

        self.do_search(&args.query, args.count).await
    }
}
