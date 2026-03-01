//! Agent lifecycle hooks for extensible agent behavior.
//!
//! Provides the [`AgentHook`] trait with default no-op implementations for each
//! lifecycle stage.  External code registers hooks via [`HookRegistry`] and the
//! agent loop calls them at the appropriate points.

pub mod logging;
pub mod persistence;
pub mod prompt;
pub mod summarization;

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::providers::{ChatMessage, ChatResponse};
use crate::session::SessionMessage;

// ── Context types ───────────────────────────────────────────

/// Context for the [`AgentHook::on_request`] callback.
///
/// Passed **after** the user message is received but **before** any processing
/// begins.  Setting `skip = true` short-circuits the entire pipeline and
/// returns an empty response.
pub struct RequestContext {
    /// Unique identifier for this request (UUID v4), stable across all hook stages.
    pub request_id: String,
    /// Session key for this conversation.
    pub session_key: String,
    /// The raw user message.
    pub user_message: String,
    /// Set to `true` to abort processing early.
    pub skip: bool,
    /// Free-form metadata propagated across hooks within a single request.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_llm_request`] callback.
///
/// Passed once per agent-loop iteration, **before** the LLM call is made.
/// Hooks may inspect or mutate the message list.
pub struct LlmRequestContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Messages that will be sent to the LLM (mutable).
    pub messages: Vec<ChatMessage>,
    /// Current iteration number (1-based).
    pub iteration: u32,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_llm_response`] callback.
///
/// Passed once per agent-loop iteration, **after** the LLM response is
/// accumulated from the stream.
pub struct LlmResponseContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// The complete LLM response.
    pub response: ChatResponse,
    /// Current iteration number (1-based).
    pub iteration: u32,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_tool_execute`] callback.
///
/// Passed **before** a single tool call is executed.  Setting `skip = true`
/// prevents execution and injects `skip_result` (or a default message) into
/// the conversation instead.
pub struct ToolExecuteContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Name of the tool about to be called.
    pub tool_name: String,
    /// Tool call arguments.
    pub tool_args: serde_json::Value,
    /// Set to `true` to skip this tool call.
    pub skip: bool,
    /// Custom result to use when `skip = true`.
    /// If `None`, a default "[skipped by hook]" message is used.
    pub skip_result: Option<String>,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_tool_result`] callback.
///
/// Passed **after** a tool call completes (or is skipped).
pub struct ToolResultContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Name of the tool that was called.
    pub tool_name: String,
    /// The tool output.
    pub tool_result: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_response`] callback.
///
/// Passed **after** the agent loop finishes but **before** the response is
/// returned to the caller.
pub struct ResponseContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Final response text.
    pub content: String,
    /// Reasoning / thinking content (if present).
    pub reasoning_content: Option<String>,
    /// Names of tools used during this request.
    pub tools_used: Vec<String>,
    /// Session key.
    pub session_key: String,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_session_load`] callback.
///
/// Passed when the agent needs to load conversation history.
/// Hooks can provide history from any source (DB, file, in-memory).
pub struct SessionLoadContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Session key.
    pub session_key: String,
    /// Maximum number of history messages to return.
    pub memory_window: usize,
    /// The loaded history (populated by hook).
    pub history: Vec<SessionMessage>,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_session_save`] callback.
///
/// Passed when the agent needs to persist a message (user or assistant).
pub struct SessionSaveContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Session key.
    pub session_key: String,
    /// Message role: "user" or "assistant".
    pub role: String,
    /// Message content.
    pub content: String,
    /// Tools used (only for assistant messages).
    pub tools_used: Option<Vec<String>>,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context for the [`AgentHook::on_context_prepare`] callback.
///
/// Passed after history truncation but before prompt assembly.
/// Hooks can inject summaries, long-term memory, or extra context.
pub struct ContextPrepareContext {
    /// Unique identifier for this request (UUID v4).
    pub request_id: String,
    /// Session key.
    pub session_key: String,
    /// Messages that were evicted from history due to token budget.
    pub evicted_messages: Vec<SessionMessage>,
    /// Hook-provided system prompt fragments.
    pub system_prompts: Vec<String>,
    /// Hook-provided summary of evicted messages.
    pub summary: Option<String>,
    /// Hook-provided long-term memory content.
    pub memory: Option<String>,
    /// Shared metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

// ── AgentHook trait ─────────────────────────────────────────

/// Extension point for the agent lifecycle.
///
/// Each method corresponds to a stage in the agent loop.  All methods have
/// default no-op implementations so implementors only need to override what
/// they care about.
///
/// # Stateless Contract
///
/// Implementations **must be stateless** with respect to cross-request mutable
/// state.  The `&self` receiver is shared across all concurrent requests via
/// `Arc<dyn AgentHook>`, and hooks must not maintain mutable data structures
/// keyed by session or request.
///
/// ## Allowed state
///
/// - **Read-only configuration** loaded at construction time (e.g., a system
///   prompt string, skills context).
/// - **Service references** to external stores (`SessionManager`, `SqliteStore`)
///   that manage their own concurrency via connection pools.
/// - **Metrics counters** using lock-free atomics (`AtomicU32`, etc.).
///
/// ## Prohibited state
///
/// - **Mutable collections** keyed by session or request
///   (e.g., `Mutex<HashMap<String, Session>>`).
/// - **Any in-memory cache** that acts as a source of truth for
///   request-scoped data.
///
/// All request-scoped information is passed through the `*Context` structs,
/// which include a stable `request_id` (UUID v4) for correlation and a
/// `metadata` HashMap for cross-hook communication within a single request.
#[async_trait::async_trait]
pub trait AgentHook: Send + Sync + Any {
    /// Downcast support for [`HookRegistry::get_hook`].
    fn as_any(&self) -> &dyn Any;

    /// Called after receiving a user request, before processing begins.
    async fn on_request(&self, _ctx: &mut RequestContext) {}

    /// Called to load session history.  The **first** hook to populate
    /// `ctx.history` wins; subsequent hooks can inspect but should not
    /// overwrite unless they intend to replace the source.
    async fn on_session_load(&self, _ctx: &mut SessionLoadContext) {}

    /// Called to persist a message (user or assistant) to storage.
    async fn on_session_save(&self, _ctx: &mut SessionSaveContext) {}

    /// Called after history truncation, before prompt assembly.
    /// Hooks can set `ctx.summary` (compressed context) and `ctx.memory`
    /// (long-term memory) to enrich the prompt.
    async fn on_context_prepare(&self, _ctx: &mut ContextPrepareContext) {}

    /// Called before each LLM request in the agent iteration loop.
    async fn on_llm_request(&self, _ctx: &mut LlmRequestContext) {}

    /// Called after each LLM response is received.
    async fn on_llm_response(&self, _ctx: &mut LlmResponseContext) {}

    /// Called before executing a single tool call.
    async fn on_tool_execute(&self, _ctx: &mut ToolExecuteContext) {}

    /// Called after a tool call completes.
    async fn on_tool_result(&self, _ctx: &mut ToolResultContext) {}

    /// Called before returning the final response to the caller.
    async fn on_response(&self, _ctx: &mut ResponseContext) {}
}

// ── HookRegistry ────────────────────────────────────────────

/// Ordered collection of [`AgentHook`] implementations.
///
/// Hooks are invoked in registration order for each lifecycle event.
pub struct HookRegistry {
    hooks: Vec<Arc<dyn AgentHook>>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook.  Hooks are called in registration order.
    ///
    /// # Contract
    ///
    /// The hook must satisfy the [Stateless Contract](AgentHook) — it must not
    /// hold mutable state keyed by session or request.  Violations will lead to
    /// data races in the gateway's concurrent processing model.
    pub fn register(&mut self, hook: Arc<dyn AgentHook>) {
        self.hooks.push(hook);
    }

    /// Whether the registry has any hooks registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Get a reference to a specific hook by its concrete type.
    ///
    /// Returns the first hook that can be downcast to `T`, or `None`.
    pub fn get_hook<T: AgentHook + 'static>(&self) -> Option<&T> {
        for hook in &self.hooks {
            if let Some(h) = hook.as_any().downcast_ref::<T>() {
                return Some(h);
            }
        }
        None
    }

    // ── Runners ─────────────────────────────────────────────

    pub async fn run_on_request(&self, ctx: &mut RequestContext) {
        for hook in &self.hooks {
            hook.on_request(ctx).await;
            if ctx.skip {
                break;
            }
        }
    }

    pub async fn run_on_llm_request(&self, ctx: &mut LlmRequestContext) {
        for hook in &self.hooks {
            hook.on_llm_request(ctx).await;
        }
    }

    pub async fn run_on_llm_response(&self, ctx: &mut LlmResponseContext) {
        for hook in &self.hooks {
            hook.on_llm_response(ctx).await;
        }
    }

    pub async fn run_on_tool_execute(&self, ctx: &mut ToolExecuteContext) {
        for hook in &self.hooks {
            hook.on_tool_execute(ctx).await;
            if ctx.skip {
                break;
            }
        }
    }

    pub async fn run_on_tool_result(&self, ctx: &mut ToolResultContext) {
        for hook in &self.hooks {
            hook.on_tool_result(ctx).await;
        }
    }

    pub async fn run_on_response(&self, ctx: &mut ResponseContext) {
        for hook in &self.hooks {
            hook.on_response(ctx).await;
        }
    }

    pub async fn run_on_session_load(&self, ctx: &mut SessionLoadContext) {
        for hook in &self.hooks {
            hook.on_session_load(ctx).await;
        }
    }

    pub async fn run_on_session_save(&self, ctx: &mut SessionSaveContext) {
        for hook in &self.hooks {
            hook.on_session_save(ctx).await;
        }
    }

    pub async fn run_on_context_prepare(&self, ctx: &mut ContextPrepareContext) {
        for hook in &self.hooks {
            hook.on_context_prepare(ctx).await;
        }
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A simple hook that counts how many times each method is called.
    struct CountingHook {
        request_count: AtomicU32,
        llm_request_count: AtomicU32,
        llm_response_count: AtomicU32,
        tool_execute_count: AtomicU32,
        tool_result_count: AtomicU32,
        response_count: AtomicU32,
    }

    impl CountingHook {
        fn new() -> Self {
            Self {
                request_count: AtomicU32::new(0),
                llm_request_count: AtomicU32::new(0),
                llm_response_count: AtomicU32::new(0),
                tool_execute_count: AtomicU32::new(0),
                tool_result_count: AtomicU32::new(0),
                response_count: AtomicU32::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentHook for CountingHook {
        fn as_any(&self) -> &dyn Any {
            self
        }
        async fn on_request(&self, _ctx: &mut RequestContext) {
            self.request_count.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_llm_request(&self, _ctx: &mut LlmRequestContext) {
            self.llm_request_count.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_llm_response(&self, _ctx: &mut LlmResponseContext) {
            self.llm_response_count.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_tool_execute(&self, _ctx: &mut ToolExecuteContext) {
            self.tool_execute_count.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_tool_result(&self, _ctx: &mut ToolResultContext) {
            self.tool_result_count.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_response(&self, _ctx: &mut ResponseContext) {
            self.response_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A hook that sets `skip = true` on requests.
    struct SkipRequestHook;

    #[async_trait::async_trait]
    impl AgentHook for SkipRequestHook {
        fn as_any(&self) -> &dyn Any {
            self
        }
        async fn on_request(&self, ctx: &mut RequestContext) {
            ctx.skip = true;
        }
    }

    /// A hook that sets `skip = true` on tool execution.
    struct SkipToolHook;

    #[async_trait::async_trait]
    impl AgentHook for SkipToolHook {
        fn as_any(&self) -> &dyn Any {
            self
        }
        async fn on_tool_execute(&self, ctx: &mut ToolExecuteContext) {
            ctx.skip = true;
            ctx.skip_result = Some("blocked by policy".to_string());
        }
    }

    /// A hook that writes metadata.
    struct MetadataHook;

    #[async_trait::async_trait]
    impl AgentHook for MetadataHook {
        fn as_any(&self) -> &dyn Any {
            self
        }
        async fn on_request(&self, ctx: &mut RequestContext) {
            ctx.metadata.insert(
                "trace_id".to_string(),
                serde_json::Value::String("abc-123".to_string()),
            );
        }
        async fn on_response(&self, ctx: &mut ResponseContext) {
            ctx.metadata
                .insert("processed".to_string(), serde_json::Value::Bool(true));
        }
    }

    fn make_request_ctx() -> RequestContext {
        RequestContext {
            request_id: "test-request-id".to_string(),
            session_key: "test:session".to_string(),
            user_message: "hello".to_string(),
            skip: false,
            metadata: HashMap::new(),
        }
    }

    fn make_tool_execute_ctx() -> ToolExecuteContext {
        ToolExecuteContext {
            request_id: "test-request-id".to_string(),
            tool_name: "read_file".to_string(),
            tool_args: serde_json::json!({"path": "/tmp/test"}),
            skip: false,
            skip_result: None,
            metadata: HashMap::new(),
        }
    }

    fn make_response_ctx() -> ResponseContext {
        ResponseContext {
            request_id: "test-request-id".to_string(),
            content: "hello back".to_string(),
            reasoning_content: None,
            tools_used: vec![],
            session_key: "test:session".to_string(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_empty_registry() {
        let registry = HookRegistry::new();
        assert!(registry.is_empty());

        let mut ctx = make_request_ctx();
        registry.run_on_request(&mut ctx).await;
        assert!(!ctx.skip);
    }

    #[tokio::test]
    async fn test_hook_called() {
        let hook = Arc::new(CountingHook::new());
        let mut registry = HookRegistry::new();
        registry.register(hook.clone());

        let mut ctx = make_request_ctx();
        registry.run_on_request(&mut ctx).await;
        assert_eq!(hook.request_count.load(Ordering::SeqCst), 1);

        registry.run_on_request(&mut ctx).await;
        assert_eq!(hook.request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_hooks_called_in_order() {
        use std::sync::Mutex;

        struct OrderTracker {
            id: u32,
            order: Arc<Mutex<Vec<u32>>>,
        }

        #[async_trait::async_trait]
        impl AgentHook for OrderTracker {
            fn as_any(&self) -> &dyn Any {
                self
            }
            async fn on_request(&self, _ctx: &mut RequestContext) {
                self.order.lock().unwrap().push(self.id);
            }
        }

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(OrderTracker {
            id: 1,
            order: order.clone(),
        }));
        registry.register(Arc::new(OrderTracker {
            id: 2,
            order: order.clone(),
        }));
        registry.register(Arc::new(OrderTracker {
            id: 3,
            order: order.clone(),
        }));

        let mut ctx = make_request_ctx();
        registry.run_on_request(&mut ctx).await;

        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_request_skip() {
        let counter = Arc::new(CountingHook::new());
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(SkipRequestHook));
        registry.register(counter.clone()); // should NOT be called

        let mut ctx = make_request_ctx();
        registry.run_on_request(&mut ctx).await;

        assert!(ctx.skip);
        // CountingHook's on_request should not have been reached
        assert_eq!(counter.request_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_tool_execute_skip() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(SkipToolHook));

        let mut ctx = make_tool_execute_ctx();
        registry.run_on_tool_execute(&mut ctx).await;

        assert!(ctx.skip);
        assert_eq!(ctx.skip_result, Some("blocked by policy".to_string()));
    }

    #[tokio::test]
    async fn test_metadata_propagation() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(MetadataHook));

        let mut req_ctx = make_request_ctx();
        registry.run_on_request(&mut req_ctx).await;
        assert_eq!(
            req_ctx.metadata.get("trace_id"),
            Some(&serde_json::Value::String("abc-123".to_string()))
        );

        let mut resp_ctx = make_response_ctx();
        registry.run_on_response(&mut resp_ctx).await;
        assert_eq!(
            resp_ctx.metadata.get("processed"),
            Some(&serde_json::Value::Bool(true))
        );
    }
}
