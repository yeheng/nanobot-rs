//! Generic rig-based LLM provider.
//!
//! `RigCompletionProvider` implements `LlmProvider` for any rig `CompletionClient`,
//! eliminating per-vendor boilerplate. Vendor-specific message normalization
//! (e.g. MiniMax) can be injected via `with_normalizer`.

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use tracing::instrument;

use crate::rig_bridge::{from_rig_response, from_rig_stream, to_rig_request};
use crate::{ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmProvider, ProviderError};

/// Generic provider wrapping any rig `CompletionClient`.
pub struct RigCompletionProvider<C> {
    name: String,
    default_model: String,
    client: C,
    normalize_fn: Option<fn(Vec<ChatMessage>) -> Vec<ChatMessage>>,
    default_max_tokens: Option<u32>,
    supports_thinking: bool,
}

impl<C> RigCompletionProvider<C> {
    /// Create a new rig-backed provider.
    pub fn new(name: impl Into<String>, default_model: impl Into<String>, client: C) -> Self {
        Self {
            name: name.into(),
            default_model: default_model.into(),
            client,
            normalize_fn: None,
            default_max_tokens: None,
            supports_thinking: false,
        }
    }

    /// Inject a message-normalization function (e.g. for MiniMax workaround).
    pub fn with_normalizer(
        mut self,
        f: fn(Vec<ChatMessage>) -> Vec<ChatMessage>,
    ) -> Self {
        self.normalize_fn = Some(f);
        self
    }

    /// Set a default `max_tokens` value injected when the request omits it.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.default_max_tokens = Some(max_tokens);
        self
    }

    /// Declare whether this provider supports thinking/reasoning mode.
    pub fn with_thinking(mut self, supports: bool) -> Self {
        self.supports_thinking = supports;
        self
    }
}

#[async_trait]
impl<C: CompletionClient + Send + Sync> LlmProvider for RigCompletionProvider<C>
where
    C::CompletionModel: CompletionModel + Send + Sync,
    <C::CompletionModel as CompletionModel>::StreamingResponse: Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn normalize_messages(&self, messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        match self.normalize_fn {
            Some(f) => f(messages),
            None => messages,
        }
    }

    fn supports_thinking(&self) -> bool {
        self.supports_thinking
    }

    #[instrument(skip(self, request), fields(provider = %self.name, model = %request.model))]
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let mut request = request;
        request.messages = self.normalize_messages(request.messages);
        if request.max_tokens.is_none() {
            if let Some(default) = self.default_max_tokens {
                request.max_tokens = Some(default);
            }
        }
        let model = self.client.completion_model(&request.model);
        let rig_request = to_rig_request(request);
        let response = model
            .completion(rig_request)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        Ok(from_rig_response(response))
    }

    #[instrument(skip(self, request), fields(provider = %self.name, model = %request.model))]
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        let mut request = request;
        request.messages = self.normalize_messages(request.messages);
        if request.max_tokens.is_none() {
            if let Some(default) = self.default_max_tokens {
                request.max_tokens = Some(default);
            }
        }
        let model = self.client.completion_model(&request.model);
        let rig_request = to_rig_request(request);
        let stream = model
            .stream(rig_request)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        Ok(from_rig_stream(stream))
    }
}
