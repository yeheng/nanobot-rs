//! Shared HTTP client builder for web tools.

use reqwest::{Client, Proxy};
use tracing::warn;

use super::ToolError;

/// Build a reqwest client with proxy configuration.
///
/// Priority order:
/// 1. Explicit proxy URLs in config (http_proxy, https_proxy, socks5_proxy)
/// 2. System environment variables (default behaviour when use_env_proxy is true)
/// 3. No proxy at all (when use_env_proxy is false and no explicit proxy is set)
pub fn build_client_with_proxy(
    config: Option<&crate::config::WebToolsConfig>,
) -> Result<Client, ToolError> {
    let mut builder = Client::builder();

    if let Some(cfg) = config {
        let has_explicit_proxy =
            cfg.http_proxy.is_some() || cfg.https_proxy.is_some() || cfg.socks5_proxy.is_some();

        if has_explicit_proxy {
            if let Some(ref proxy_url) = cfg.http_proxy {
                match Proxy::http(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid HTTP proxy URL '{}': {}", proxy_url, e),
                }
            }

            if let Some(ref proxy_url) = cfg.https_proxy {
                match Proxy::https(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid HTTPS proxy URL '{}': {}", proxy_url, e),
                }
            }

            if let Some(ref proxy_url) = cfg.socks5_proxy {
                match Proxy::all(proxy_url) {
                    Ok(proxy) => builder = builder.proxy(proxy),
                    Err(e) => warn!("Invalid SOCKS5 proxy URL '{}': {}", proxy_url, e),
                }
            }
        } else if !cfg.use_env_proxy {
            builder = builder.no_proxy();
        }
    }

    builder
        .build()
        .map_err(|e| ToolError::ExecutionError(format!("Failed to create HTTP client: {}", e)))
}
