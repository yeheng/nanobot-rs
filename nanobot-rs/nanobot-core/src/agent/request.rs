//! Request handling utilities for the agent.
//!
//! Provides LLM request building and retry logic with exponential backoff.

use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use crate::providers::{ChatRequest, ChatStream, LlmProvider, ThinkingConfig};
use crate::tools::ToolRegistry;

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
    pub fn build_chat_request(&self, messages: &[crate::providers::ChatMessage]) -> ChatRequest {
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

    /// Send a request to the provider with exponential-backoff retries.
    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream> {
        let mut retries = 0u32;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    if retries >= MAX_RETRIES {
                        return Err(e.context("Provider request failed after retries"));
                    }
                    retries += 1;
                    warn!(
                        "Provider error: {}. Retrying {}/{}",
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
}
