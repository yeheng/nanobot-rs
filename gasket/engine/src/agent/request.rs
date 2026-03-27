//! Request handling utilities for the agent.
//!
//! Provides LLM request building and retry logic with exponential backoff.

use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use crate::tools::ToolRegistry;
use gasket_providers::{ChatRequest, ChatStream, LlmProvider, ThinkingConfig};

use super::loop_::AgentConfig;

/// Maximum retries for transient provider errors.
const MAX_RETRIES: u32 = 3;

/// Handler for LLM requests with retry support.
pub struct RequestHandler<'a> {
    provider: &'a Arc<dyn LlmProvider>,
    tools: &'a ToolRegistry,
    config: &'a AgentConfig,
}

impl<'a> RequestHandler<'a> {
    /// Create a new request handler.
    pub fn new(
        provider: &'a Arc<dyn LlmProvider>,
        tools: &'a ToolRegistry,
        config: &'a AgentConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

    /// Build a `ChatRequest` from the current config and messages.
    pub fn build_chat_request(&self, messages: &[gasket_providers::ChatMessage]) -> ChatRequest {
        ChatRequest {
            model: self.config.model.clone(),
            messages: messages.to_vec(),
            tools: Some(self.tools.get_definitions()),
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            thinking: if self.config.thinking_enabled {
                Some(ThinkingConfig::enabled())
            } else {
                None
            },
        }
    }

    /// Determine if an error is retryable.
    ///
    /// Retryable errors include:
    /// - Network errors (connection timeout, DNS failure, etc.)
    /// - HTTP 429 (Rate Limit)
    /// - HTTP 5xx (Server Error)
    ///
    /// Non-retryable errors include:
    /// - HTTP 400 (Bad Request) - client error
    /// - HTTP 401 (Unauthorized) - authentication error
    /// - HTTP 403 (Forbidden) - permission denied
    /// - HTTP 404 (Not Found) - resource not found
    fn is_retryable_error(error: &anyhow::Error) -> bool {
        let error_str = error.to_string().to_lowercase();

        // Check for HTTP status codes in error message
        // Format: "provider API error: XXX - ..."
        if let Some(status_code) = extract_http_status(&error_str) {
            return is_retryable_http_status(status_code);
        }

        // Check for network-related errors
        let network_error_patterns = [
            "connection refused",
            "connection reset",
            "connection timed out",
            "timed out",
            "timeout",
            "dns error",
            "name resolution failed",
            "no route to host",
            "network unreachable",
            "broken pipe",
            "unexpected eof",
            "ssl error",
            "tls error",
            "certificate",
            "hyper::error", // HTTP client errors
        ];

        for pattern in &network_error_patterns {
            if error_str.contains(pattern) {
                return true;
            }
        }

        // Default to not retrying for unknown errors
        false
    }

    /// Send a request to the provider with exponential-backoff retries.
    ///
    /// Only retries on transient errors (network issues, rate limits, server errors).
    /// Client errors (4xx except 429) fail immediately without retry.
    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream> {
        let mut retries = 0u32;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    // Check if error is retryable
                    if !Self::is_retryable_error(&e) {
                        // Non-retryable error: fail immediately
                        return Err(e.context("Provider request failed (non-retryable)"));
                    }

                    if retries >= MAX_RETRIES {
                        return Err(e.context("Provider request failed after retries"));
                    }
                    retries += 1;
                    warn!(
                        "Provider error (retryable): {}. Retrying {}/{}",
                        e, retries, MAX_RETRIES
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2_u64.pow(retries))).await;
                }
            }
        }
    }
}

/// Extract HTTP status code from error message.
///
/// Looks for patterns like "401 unauthorized", "500 internal server error", etc.
fn extract_http_status(error_str: &str) -> Option<u16> {
    // Common patterns in error messages:
    // - "API error: 401 - ..."
    // - "status: 500"
    // - "http 429"

    // Look for "API error: XXX" pattern
    if let Some(pos) = error_str.find("api error:") {
        let after = &error_str[pos + 10..].trim_start();
        if let Some(space_pos) = after.find(|c: char| !c.is_ascii_digit()) {
            if let Ok(code) = after[..space_pos].parse::<u16>() {
                return Some(code);
            }
        }
    }

    // Look for "status XXX" pattern
    if let Some(pos) = error_str.find("status ") {
        let after = &error_str[pos + 7..];
        if let Some(space_pos) = after.find(|c: char| !c.is_ascii_digit()) {
            if let Ok(code) = after[..space_pos].parse::<u16>() {
                return Some(code);
            }
        }
    }

    None
}

/// Check if an HTTP status code indicates a retryable error.
fn is_retryable_http_status(status: u16) -> bool {
    match status {
        // Client errors: never retry (except 429)
        400..=428 => false,
        // Rate limit: retry with backoff
        429 => true,
        // Other client errors: never retry
        430..=499 => false,
        // Server errors: retry
        500..=599 => true,
        // Other: don't retry
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    #[test]
    fn test_is_retryable_http_status() {
        // Non-retryable client errors
        assert!(!is_retryable_http_status(400));
        assert!(!is_retryable_http_status(401));
        assert!(!is_retryable_http_status(403));
        assert!(!is_retryable_http_status(404));
        assert!(!is_retryable_http_status(422));

        // Retryable: rate limit
        assert!(is_retryable_http_status(429));

        // Retryable: server errors
        assert!(is_retryable_http_status(500));
        assert!(is_retryable_http_status(502));
        assert!(is_retryable_http_status(503));
        assert!(is_retryable_http_status(504));
    }

    #[test]
    fn test_extract_http_status() {
        assert_eq!(
            extract_http_status("openai api error: 401 - unauthorized"),
            Some(401)
        );
        assert_eq!(
            extract_http_status("provider api error: 500 - internal server error"),
            Some(500)
        );
        assert_eq!(extract_http_status("status 429 - rate limited"), Some(429));
        assert_eq!(extract_http_status("random error without status"), None);
    }

    #[test]
    fn test_is_retryable_error_network() {
        // Network errors should be retryable
        let err = anyhow::anyhow!("connection timed out");
        assert!(RequestHandler::is_retryable_error(&err));

        let err = anyhow::anyhow!("dns error: name resolution failed");
        assert!(RequestHandler::is_retryable_error(&err));

        // Non-retryable HTTP errors
        let err = anyhow::anyhow!("openai api error: 401 - unauthorized");
        assert!(!RequestHandler::is_retryable_error(&err));

        let err = anyhow::anyhow!("api error: 404 - model not found");
        assert!(!RequestHandler::is_retryable_error(&err));

        // Retryable HTTP errors
        let err = anyhow::anyhow!("api error: 429 - rate limited");
        assert!(RequestHandler::is_retryable_error(&err));

        let err = anyhow::anyhow!("api error: 503 - service unavailable");
        assert!(RequestHandler::is_retryable_error(&err));
    }
}
