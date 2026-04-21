//! LLM request handler with retry support.

use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use crate::kernel::context::KernelConfig;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatRequest, ChatStream, LlmProvider, ThinkingConfig};

/// Handler for LLM requests with retry support.
pub struct RequestHandler<'a> {
    provider: &'a Arc<dyn LlmProvider>,
    tools: &'a ToolRegistry,
    config: &'a KernelConfig,
}

impl<'a> RequestHandler<'a> {
    pub fn new(
        provider: &'a Arc<dyn LlmProvider>,
        tools: &'a ToolRegistry,
        config: &'a KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

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

    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream> {
        let mut retries = 0u32;
        let max_retries = self.config.max_retries;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(provider_err) => {
                    let e = anyhow::anyhow!("{}", provider_err);
                    if !provider_err.is_retryable() {
                        return Err(e.context("Provider request failed (non-retryable)"));
                    }

                    if retries >= max_retries {
                        return Err(e.context("Provider request failed after retries"));
                    }
                    retries += 1;
                    warn!(
                        "Provider error (retryable): {}. Retrying {}/{}",
                        e, retries, max_retries
                    );
                    let backoff_secs = (1u64 << retries.min(63)).min(15);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    }
}
