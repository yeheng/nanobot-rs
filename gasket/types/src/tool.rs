//! Base trait for tools
//!
//! This module defines the core `Tool` trait and related types that all
//! gasket tools must implement. By keeping these types in `gasket-types`,
//! we avoid circular dependencies between `gasket-core` and `gasket-tools`.

use async_trait::async_trait;
use serde_json::Value;

use crate::events::{OutboundMessage, SessionKey};

/// Result type for tool execution
pub type ToolResult = Result<String, ToolError>;

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
    ///
    /// # Returns
    /// The subagent result or an error if spawning fails
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>>;

    /// Spawn a subagent with a real-time event stream.
    ///
    /// Returns:
    /// - `String`: the subagent ID (UUID)
    /// - `Receiver<StreamEvent>`: real-time events (thinking, tool calls, content)
    /// - `Receiver<SubagentResult>`: final result when execution completes
    ///
    /// Default implementation delegates to [`spawn`](Self::spawn) and returns
    /// empty channels for backward compatibility.
    async fn spawn_with_stream(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<
        (
            String,
            tokio::sync::mpsc::Receiver<crate::StreamEvent>,
            tokio::sync::oneshot::Receiver<SubagentResult>,
        ),
        Box<dyn std::error::Error + Send>,
    > {
        let result = self.spawn(task, model_id).await?;
        let (_, rx) = tokio::sync::mpsc::channel(1);
        let (tx, result_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(result);
        Ok((String::new(), rx, result_rx))
    }
}

/// No-op spawner that always returns an error.
///
/// Used as the default `ToolContext::spawner` when no real spawner is available
/// (e.g., in CLI mode or unit tests). This eliminates `Option` wrapping and
/// ensures `SpawnTool` gets a clear runtime error instead of a `None` panic.
pub struct NoopSpawner;

#[async_trait]
impl SubagentSpawner for NoopSpawner {
    async fn spawn(
        &self,
        _task: String,
        _model_id: Option<String>,
    ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Subagent spawning is not available in this context",
        )))
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
    pub token_usage: Option<TokenUsage>,
    pub cost: f64,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Callback for synthesizing subagent results into a final response.
///
/// The concrete implementation holds provider, outbound_tx, session_key etc.
/// Returned Future is 'static so it can be safely moved into a tokio::spawn task.
pub trait SynthesisCallback: Send + Sync {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send>>> + Send>>;
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
    /// Subagent spawner for tools that need to spawn subagents.
    /// Always present — defaults to `NoopSpawner` when spawning is unavailable.
    pub spawner: std::sync::Arc<dyn SubagentSpawner>,
    /// Token tracker for budget enforcement across parent and subagents.
    /// Always present — defaults to an unlimited tracker when not configured.
    pub token_tracker: std::sync::Arc<crate::token_tracker::TokenTracker>,
    /// Maximum characters for WebSocket subagent summary (0 = unlimited).
    pub ws_summary_limit: usize,
    /// Callback for triggering synthesis after all subagents complete.
    /// When None (CLI/Telegram/non-WebSocket mode), spawn tools use blocking mode.
    pub synthesis_callback: Option<std::sync::Arc<dyn SynthesisCallback>>,
}

impl Default for ToolContext {
    fn default() -> Self {
        let (outbound_tx, _rx) = tokio::sync::mpsc::channel(1);
        Self {
            session_key: SessionKey::new(crate::events::ChannelType::Cli, "default"),
            outbound_tx,
            spawner: std::sync::Arc::new(NoopSpawner),
            token_tracker: std::sync::Arc::new(crate::token_tracker::TokenTracker::default()),
            ws_summary_limit: 0,
            synthesis_callback: None,
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
            .field("synthesis_callback", &self.synthesis_callback.is_some())
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
        self.spawner = s;
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

    pub fn synthesis_callback(mut self, cb: std::sync::Arc<dyn SynthesisCallback>) -> Self {
        self.synthesis_callback = Some(cb);
        self
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

    /// Deep-clone this tool into a new boxed trait object.
    ///
    /// Tools that hold mutable state (e.g. engine references) should override
    /// this so that each `ToolRegistry` clone receives an independent copy.
    /// Stateless tools can rely on the default `None`, in which case the
    /// registry will fall back to a cheap `Arc` clone.
    fn clone_box(&self) -> Option<Box<dyn Tool>> {
        None
    }
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
