//! Kernel context: dependencies needed for the pure LLM execution loop.

use std::sync::Arc;

use crate::tools::{ToolContext, ToolRegistry};
use async_trait::async_trait;
use gasket_providers::LlmProvider;
use gasket_types::SessionRefs;

/// Async callback for proactive working-memory checkpoint injection.
///
/// Called before each `step()` with the current message count; returns
/// a summary to inject, or `None` to skip.
///
/// This is a kernel-level extension point (fires per iteration inside the
/// step loop), distinct from session-level `PipelineHook`s (fire once per
/// request during context construction).
#[async_trait]
pub trait CheckpointCallback: Send + Sync {
    async fn get_checkpoint(&self, msg_len: usize) -> Option<String>;
}

/// Everything the kernel needs to execute one LLM request.
/// Passed by reference to `kernel::execute()` — no ownership.
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub role: gasket_types::AgentRole,
    /// Optional checkpoint callback for proactive working-memory injection.
    pub checkpoint_callback: Option<Arc<dyn CheckpointCallback>>,
    /// Session-level references shared with ToolContext.
    pub refs: SessionRefs,
}

impl RuntimeContext {
    /// Internal constructor — public API delegates to this with the desired role.
    fn new_with_role(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
        role: gasket_types::AgentRole,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            role,
            checkpoint_callback: None,
            refs: SessionRefs::default(),
        }
    }

    /// Constructs an Orchestrator context (main agent, may attach a spawner).
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self::new_with_role(provider, tools, config, gasket_types::AgentRole::Orchestrator)
    }

    /// Constructs a Worker context (subagent leaf). `spawner` is forced to None
    /// to enforce the type invariant: workers cannot dispatch further workers.
    pub fn new_worker(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self::new_with_role(provider, tools, config, gasket_types::AgentRole::Worker)
    }

    /// Build a `ToolContext` from this RuntimeContext.
    ///
    /// Applies session refs (spawner, session_key, etc.) onto a `ToolContext`
    /// pre-filled with kernel config values. This is the single mapping point
    /// between kernel context and tool context.
    pub fn build_tool_context(&self) -> ToolContext {
        use crate::kernel::synthesis::WebSocketSynthesizer;

        let mut ctx = ToolContext::default()
            .ws_summary_limit(self.config.ws_summary_limit)
            .plugin_timeout_secs(self.config.plugin_timeout_secs);

        ctx.apply_session_refs(&self.refs);

        // Inject SynthesisCallback when outbound channel is present (WebSocket mode).
        if let Some(outbound_tx) = self.refs.outbound_tx.clone() {
            let provider = &self.provider;
            let model = provider.default_model().to_string();
            let session_key = self.refs.session_key.clone().unwrap_or_else(|| {
                gasket_types::SessionKey::new(gasket_types::events::ChannelType::Cli, "default")
            });
            let callback = std::sync::Arc::new(WebSocketSynthesizer::new(
                provider.clone(),
                model,
                outbound_tx,
                session_key,
            ));
            ctx = ctx.synthesis_callback(callback);
        }

        ctx
    }
}

impl Clone for RuntimeContext {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: self.config.clone(),
            role: self.role,
            checkpoint_callback: self.checkpoint_callback.clone(),
            refs: self.refs.clone(),
        }
    }
}

/// Minimal config for the LLM iteration loop.
#[derive(Clone)]
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub max_retries: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
    pub tool_timeout_secs: u64,
    /// Plugin execution timeout in seconds (fallback when manifest omits it).
    pub plugin_timeout_secs: u64,
    /// Maximum characters for WebSocket subagent summary (0 = unlimited).
    pub ws_summary_limit: usize,
    /// Optional whitelist of tool names visible to the LLM for this run.
    /// `None` exposes all registered tools; `Some(vec![])` forbids all tools.
    pub tool_filter: Option<Vec<String>>,
}

// Stream forwarding limits — fixed defensive bounds, not user-tunable.
//
// These prevent runaway streams from hanging the kernel loop. Nobody has
// ever needed to override them; demoting them to constants keeps the
// `KernelConfig` surface honest.
/// Per-chunk timeout for the streaming forwarding loop.
pub const STREAM_CHUNK_TIMEOUT_SECS: u64 = 120;
/// Hard cap on stream chunks per LLM turn to prevent hangs.
pub const MAX_STREAM_CHUNKS: usize = 100_000;
/// Maximum number of tool calls executed concurrently.
pub const TOOL_CONCURRENCY: usize = 5;

impl KernelConfig {
    pub fn new(model: String) -> Self {
        Self {
            model,
            max_iterations: 100,
            max_retries: 3,
            temperature: 1.0,
            max_tokens: 65536,
            max_tool_result_chars: 16000,
            thinking_enabled: false,
            tool_timeout_secs: 120,
            plugin_timeout_secs: 120,
            ws_summary_limit: 0,
            tool_filter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_config_default() {
        let config = KernelConfig::new("test-model".to_string());
        assert_eq!(config.model, "test-model");
        assert_eq!(config.max_iterations, 100);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn role_default_is_orchestrator() {
        use gasket_types::AgentRole;
        assert_eq!(default_role(), AgentRole::Orchestrator);
    }

    fn default_role() -> gasket_types::AgentRole {
        gasket_types::AgentRole::Orchestrator
    }
}
