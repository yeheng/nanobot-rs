//! Request handling utilities for the agent.
//!
//! Provides LLM request building and retry logic with exponential backoff.

use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use crate::tools::ToolRegistry;
use gasket_providers::{ChatRequest, ChatStream, LlmProvider, ProviderError, ThinkingConfig};

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
    #[allow(dead_code)]
    fn is_retryable_error(error: &anyhow::Error) -> bool {
        if let Some(provider_err) = error.downcast_ref::<ProviderError>() {
            return provider_err.is_retryable();
        }

        let error_str = error.to_string().to_lowercase();

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
            "hyper::error",
        ];

        for pattern in &network_error_patterns {
            if error_str.contains(pattern) {
                return true;
            }
        }

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
                Err(provider_err) => {
                    let e = anyhow::anyhow!("{}", provider_err);
                    // Check if error is retryable
                    if !provider_err.is_retryable() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    #[test]
    fn test_is_retryable_error_network() {
        let err = anyhow::anyhow!("connection timed out");
        assert!(RequestHandler::is_retryable_error(&err));

        let err = anyhow::anyhow!("dns error: name resolution failed");
        assert!(RequestHandler::is_retryable_error(&err));
    }
}
