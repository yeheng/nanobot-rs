//! LLM request handler with retry support.

use std::sync::Arc;

use tracing::warn;

use crate::kernel::context::KernelConfig;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatRequest, ChatStream, LlmProvider, ProviderError, ThinkingConfig};

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
        let request = ChatRequest {
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
        };
        if let Ok(json) = serde_json::to_string(&request) {
            tracing::debug!("[RequestHandler] built request: {}", json);
        }
        request
    }

    pub async fn send_with_retry(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        let mut retries = 0u32;
        let max_retries = self.config.max_retries;
        loop {
            match self.provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(provider_err) => {
                    if !provider_err.is_retryable() {
                        return Err(provider_err);
                    }

                    if retries >= max_retries {
                        return Err(provider_err);
                    }
                    retries += 1;
                    warn!(
                        "Provider error (retryable): {}. Retrying {}/{}",
                        provider_err, retries, max_retries
                    );
                    let backoff_secs = (1u64 << retries.min(63)).min(15);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    }
}
