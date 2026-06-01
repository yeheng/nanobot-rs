//! Base trait for tools
//!
//! This module defines the core `Tool` trait and related types that all
//! gasket tools must implement. By keeping these types in `gasket-types`,
//! we avoid circular dependencies between `gasket-core` and `gasket-tools`.

use async_trait::async_trait;
use serde_json::Value;

use std::sync::Arc;

use crate::events::{OutboundMessage, SessionKey};
use crate::pending_ask::DynPendingAskRegistry;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

/// Shared handle for the "cancel previous aggregator" pattern used by spawn tools.
///
/// Holds at most one live `CancellationToken`. When a new aggregator launches,
/// it calls `swap_and_cancel_old(new)`: the previous token (if any) is cancelled
/// and replaced atomically under a single lock.
///
/// Replaces the prior `Arc<Mutex<Option<CancellationToken>>>` open-coded dance,
/// which leaked the nesting and forced every call site to repeat the same
/// take/cancel/store sequence.
#[derive(Clone, Default)]
pub struct AggregatorCancel {
    inner: Arc<Mutex<Option<CancellationToken>>>,
}

impl AggregatorCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `new` as the active token, cancelling the previous one (if any).
    ///
    /// Uses `try_lock`: if contention occurs the previous token is left intact
    /// — matching the original best-effort semantics.
    pub fn swap_and_cancel_old(&self, new: CancellationToken) {
        let mut guard = self.inner.lock();
        if let Some(old) = guard.take() {
            old.cancel();
        }
        *guard = Some(new);
    }

    /// Cancel and forget the current token, if any.
    pub fn cancel_current(&self) {
        let mut guard = self.inner.lock();
        if let Some(old) = guard.take() {
            old.cancel();
        }
    }
}

impl std::fmt::Debug for AggregatorCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregatorCancel").finish()
    }
}

/// Result type for tool execution
pub type ToolResult = Result<String, ToolError>;

/// Future type returned by [`SynthesisCallback::synthesize`].
pub type SynthesisFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send>>> + Send>,
>;

/// Trait for spawning subagents without hard dependency on SubagentManager.
///
/// This trait is defined in gasket-types to avoid circular dependencies.
/// Tools depend only on this trait, not on the concrete SubagentManager type.
#[async_trait]
pub trait SubagentSpawner: Send + Sync {
    /// Spawn a subagent with the given task and optional model selection.
    ///
    /// # Arguments
    /// * `task` - The task description for the subagent to execute
    /// * `model_id` - Optional model profile ID to use (uses default if None)
    /// * `ctx` - The caller's tool context, used to propagate session_key,
    ///   outbound_tx, and pending_asks into the subagent's RuntimeContext.
    ///
    /// # Returns
    /// The subagent result or an error if spawning fails
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
        ctx: &ToolContext,
    ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>>;

    /// Spawn a subagent with a real-time event stream.
    ///
    /// Returns:
    /// - `String`: the subagent ID (UUID)
    /// - `Receiver<StreamEvent>`: real-time events (thinking, tool calls, content)
    /// - `Receiver<SubagentResult>`: final result when execution completes
    /// - `CancellationToken`: handle to cancel the subagent mid-flight
    ///
    /// Default implementation delegates to [`spawn`](Self::spawn) and returns
    /// empty channels plus a dummy cancellation token for backward compatibility.
    async fn spawn_with_stream(
        &self,
        task: String,
        model_id: Option<String>,
        ctx: &ToolContext,
        _tool_filter: Option<Vec<String>>,
    ) -> Result<
        (
            String,
            tokio::sync::mpsc::Receiver<crate::StreamEvent>,
            tokio::sync::oneshot::Receiver<SubagentResult>,
            tokio_util::sync::CancellationToken,
        ),
        Box<dyn std::error::Error + Send>,
    > {
        let result = self.spawn(task, model_id, ctx).await?;
        let (_, rx) = tokio::sync::mpsc::channel(1);
        let (tx, result_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(result);
        Ok((
            String::new(),
            rx,
            result_rx,
            tokio_util::sync::CancellationToken::new(),
        ))
    }
}

/// Subagent execution result.
///
/// This is a minimal result type containing only what tools need.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub id: String,
    pub task: String,
    pub response: SubagentResponse,
    /// Model name used for this execution
    pub model: Option<String>,
}

/// Subagent response structure.
#[derive(Debug, Clone)]
pub struct SubagentResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub tools_used: Vec<String>,
    pub model: Option<String>,
    pub token_usage: Option<crate::token_tracker::TokenUsage>,
    pub cost: f64,
}

/// Callback for synthesizing subagent results into a final response.
///
/// The concrete implementation holds provider, outbound_tx, session_key etc.
/// Returned Future is 'static so it can be safely moved into a tokio::spawn task.
pub trait SynthesisCallback: Send + Sync {
    fn synthesize(&self, results: Vec<SubagentResult>) -> SynthesisFuture;
}

/// Session-level references shared between kernel's `RuntimeContext` and
/// the tool execution `ToolContext`.
///
/// Groups the 6 fields that both contexts need, so adding a new session-scoped
/// reference only requires changing this struct + `apply_to_tool_context()`,
/// not three separate locations.
#[derive(Clone, Default)]
pub struct SessionRefs {
    pub session_key: Option<SessionKey>,
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<OutboundMessage>>,
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    pub aggregator_cancel: Option<AggregatorCancel>,
    pub pending_asks: Option<DynPendingAskRegistry>,
    /// Optional synthesis callback. When `Some`, spawn tools operate in
    /// non-blocking mode and delegate result aggregation to this callback.
    /// Session/gateway layers decide which concrete implementation to
    /// inject — the kernel only forwards the value.
    pub synthesis_callback: Option<Arc<dyn SynthesisCallback>>,
}

impl std::fmt::Debug for SessionRefs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRefs")
            .field("session_key", &self.session_key)
            .field("outbound_tx", &"Sender<OutboundMessage>")
            .field("spawner", &self.spawner.is_some())
            .field("token_tracker", &self.token_tracker.is_some())
            .field("aggregator_cancel", &self.aggregator_cancel.is_some())
            .field("pending_asks", &self.pending_asks.is_some())
            .field("synthesis_callback", &self.synthesis_callback.is_some())
            .finish()
    }
}

/// Context passed to tool execution, providing request-scoped data.
///
/// This replaces the old pattern of storing session_key in SubagentManager
/// as a global mutable state. Now each tool execution receives its context
/// directly, eliminating multi-tenant data leakage risks.
#[derive(Clone)]
pub struct ToolContext {
    /// Session key for WebSocket streaming (identifies the client connection).
    pub session_key: SessionKey,
    /// Channel to send outbound WebSocket messages in real-time.
    pub outbound_tx: tokio::sync::mpsc::Sender<OutboundMessage>,
    /// Subagent spawner. `None` = this context cannot spawn workers
    /// (e.g. CLI mode, unit tests, or any Worker context).
    pub spawner: Option<std::sync::Arc<dyn SubagentSpawner>>,
    /// Token tracker for budget enforcement across parent and subagents.
    /// Always present — defaults to an unlimited tracker when not configured.
    pub token_tracker: std::sync::Arc<crate::token_tracker::TokenTracker>,
    /// Maximum characters for WebSocket subagent summary (0 = unlimited).
    pub ws_summary_limit: usize,
    /// Plugin execution timeout in seconds (fallback when manifest omits it).
    pub plugin_timeout_secs: u64,
    /// Callback for triggering synthesis after all subagents complete.
    /// When None (CLI/Telegram/non-WebSocket mode), spawn tools use blocking mode.
    pub synthesis_callback: Option<std::sync::Arc<dyn SynthesisCallback>>,
    /// Shared cancellation handle for the current aggregator task.
    /// Tools use this to cancel previous aggregators when spawning new ones.
    pub aggregator_cancel: Option<AggregatorCancel>,
    /// Pending-ask registry for the `ask_user` tool. None in contexts that
    /// don't need user prompting (CLI white-box, unit tests).
    pub pending_asks: Option<DynPendingAskRegistry>,
}

impl Default for ToolContext {
    fn default() -> Self {
        let (outbound_tx, _rx) = tokio::sync::mpsc::channel(1);
        Self {
            session_key: SessionKey::new(crate::events::ChannelType::Cli, "default"),
            outbound_tx,
            spawner: None,
            token_tracker: std::sync::Arc::new(crate::token_tracker::TokenTracker::default()),
            ws_summary_limit: 0,
            plugin_timeout_secs: 120,
            synthesis_callback: None,
            aggregator_cancel: None,
            pending_asks: None,
        }
    }
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("session_key", &self.session_key)
            .field("outbound_tx", &"Sender<OutboundMessage>")
            .field("spawner", &"SubagentSpawner")
            .field("token_tracker", &"TokenTracker")
            .field("ws_summary_limit", &self.ws_summary_limit)
            .field("plugin_timeout_secs", &self.plugin_timeout_secs)
            .field("synthesis_callback", &self.synthesis_callback.is_some())
            .field("aggregator_cancel", &self.aggregator_cancel.is_some())
            .field("pending_asks", &self.pending_asks.is_some())
            .finish()
    }
}

impl ToolContext {
    pub fn session_key(mut self, key: SessionKey) -> Self {
        self.session_key = key;
        self
    }

    pub fn outbound_tx(mut self, tx: tokio::sync::mpsc::Sender<OutboundMessage>) -> Self {
        self.outbound_tx = tx;
        self
    }

    pub fn spawner(mut self, s: std::sync::Arc<dyn SubagentSpawner>) -> Self {
        self.spawner = Some(s);
        self
    }

    pub fn token_tracker(
        mut self,
        tracker: std::sync::Arc<crate::token_tracker::TokenTracker>,
    ) -> Self {
        self.token_tracker = tracker;
        self
    }

    pub fn ws_summary_limit(mut self, limit: usize) -> Self {
        self.ws_summary_limit = limit;
        self
    }

    pub fn plugin_timeout_secs(mut self, secs: u64) -> Self {
        self.plugin_timeout_secs = secs;
        self
    }

    pub fn synthesis_callback(mut self, cb: std::sync::Arc<dyn SynthesisCallback>) -> Self {
        self.synthesis_callback = Some(cb);
        self
    }

    pub fn aggregator_cancel(mut self, cancel: AggregatorCancel) -> Self {
        self.aggregator_cancel = Some(cancel);
        self
    }

    pub fn pending_asks(mut self, registry: DynPendingAskRegistry) -> Self {
        self.pending_asks = Some(registry);
        self
    }

    /// Apply session-level references from a `SessionRefs` bundle.
    ///
    /// For each field that is `Some` in `refs`, overrides the current value.
    /// `None` fields are left unchanged (keeping defaults or previously set values).
    pub fn apply_session_refs(&mut self, refs: &SessionRefs) {
        if let Some(ref sk) = refs.session_key {
            self.session_key = sk.clone();
        }
        if let Some(ref tx) = refs.outbound_tx {
            self.outbound_tx = tx.clone();
        }
        if let Some(ref spawner) = refs.spawner {
            self.spawner = Some(spawner.clone());
        }
        if let Some(ref tracker) = refs.token_tracker {
            self.token_tracker = tracker.clone();
        }
        if let Some(ref cancel) = refs.aggregator_cancel {
            self.aggregator_cancel = Some(cancel.clone());
        }
        if let Some(ref registry) = refs.pending_asks {
            self.pending_asks = Some(registry.clone());
        }
        if let Some(ref cb) = refs.synthesis_callback {
            self.synthesis_callback = Some(cb.clone());
        }
    }
}

/// Error type for tool execution
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Not found: {0}")]
    NotFound(String),
}

/// Tool trait for implementing agent tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool description
    fn description(&self) -> &str;

    /// Get the JSON schema for parameters
    fn parameters(&self) -> Value;

    /// Execute the tool with given arguments and context.
    ///
    /// The `ctx` parameter provides request-scoped data like session_key
    /// and outbound channel for WebSocket streaming. This eliminates the
    /// need for global mutable state in SubagentManager.
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;

    /// Return as `&dyn Any` for downcasting.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Metadata describing a tool's capabilities, tags, and permission requirements.
#[derive(Debug, Clone, Default)]
pub struct ToolMetadata {
    /// Human-readable display name.
    pub display_name: String,

    /// Category (e.g., "filesystem", "network", "shell").
    pub category: String,

    /// Tags for filtering and discovery.
    pub tags: Vec<String>,

    /// Whether this tool requires explicit user approval.
    pub requires_approval: bool,

    /// Whether this tool can modify external state.
    pub is_mutating: bool,
}

/// Helper to create a simple JSON schema for tool parameters.
///
/// Each entry is `(name, type, required, description)`.
///
/// Supported type formats:
/// - `"string"`, `"integer"`, `"number"`, `"boolean"` - basic types
/// - `"array"` - array of strings (default element type)
/// - `"array<T>"` - array with specific element type (e.g., `"array<integer>"`)
/// - `"object"` - generic object (no nested properties defined)
///
/// Note: OpenAI/GPT API requires `items` field for array types.
/// This function automatically adds `{"type": "string"}` as default items schema.
pub fn simple_schema(properties: &[(&str, &str, bool, &str)]) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();

    for (name, type_desc, is_required, description) in properties {
        let prop = build_property_schema(type_desc, description);
        props.insert(name.to_string(), Value::Object(prop));

        if *is_required {
            required.push(name.to_string());
        }
    }

    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required
    })
}

/// Build a property schema from type descriptor and description.
fn build_property_schema(type_desc: &str, description: &str) -> serde_json::Map<String, Value> {
    let mut prop = serde_json::Map::new();

    // Handle array types with optional element type: "array" or "array<T>"
    if type_desc == "array" {
        prop.insert("type".to_string(), Value::String("array".to_string()));
        prop.insert("items".to_string(), serde_json::json!({"type": "string"}));
    } else if let Some(inner) = type_desc
        .strip_prefix("array<")
        .and_then(|s| s.strip_suffix('>'))
    {
        prop.insert("type".to_string(), Value::String("array".to_string()));
        prop.insert("items".to_string(), serde_json::json!({"type": inner}));
    } else {
        // For all other types, use type as-is
        prop.insert("type".to_string(), Value::String(type_desc.to_string()));
    }

    prop.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );

    prop
}

// ── Approval system types ───────────────────────────────────

/// Request sent to the user asking for approval of a sensitive tool call.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolApprovalRequest {
    /// Unique request ID (UUID v4).
    pub id: String,
    /// Name of the tool being invoked.
    pub tool_name: String,
    /// Human-readable description of the operation.
    pub description: String,
    /// JSON-serialized tool arguments.
    pub arguments: String,
}

/// Response from the user to an approval request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolApprovalResponse {
    /// ID of the request being responded to.
    pub request_id: String,
    /// Whether the user approved the operation.
    pub approved: bool,
    /// Whether to remember this decision for future similar operations.
    #[serde(default)]
    pub remember: bool,
}

/// Callback invoked by `ToolRegistry` when a tool with `requires_approval`
/// is about to be executed.
#[async_trait]
pub trait ApprovalCallback: Send + Sync {
    /// Ask the user for approval.
    ///
    /// Returns `true` if the user approves, `false` if denied.
    /// Implementations should block (await) until the user responds or a
    /// timeout expires.
    async fn request_approval(
        &self,
        session_key: &SessionKey,
        request: ToolApprovalRequest,
    ) -> Result<bool, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tool_context_has_no_spawner() {
        let ctx = ToolContext::default();
        assert!(
            ctx.spawner.is_none(),
            "default ToolContext must not auto-attach a spawner"
        );
    }
}
