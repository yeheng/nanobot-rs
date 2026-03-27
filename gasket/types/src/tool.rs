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

/// Context passed to tool execution, providing request-scoped data.
///
/// This replaces the old pattern of storing session_key in SubagentManager
/// as a global mutable state. Now each tool execution receives its context
/// directly, eliminating multi-tenant data leakage risks.
#[derive(Clone, Default)]
pub struct ToolContext {
    /// Session key for WebSocket streaming (identifies the client connection).
    pub session_key: Option<SessionKey>,
    /// Channel to send outbound WebSocket messages in real-time.
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<OutboundMessage>>,
    /// Subagent spawner for tools that need to spawn subagents.
    /// Uses a trait object to decouple tools from concrete SubagentManager.
    pub spawner: Option<std::sync::Arc<dyn SubagentSpawner>>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("session_key", &self.session_key)
            .field("outbound_tx", &self.outbound_tx.is_some())
            .field("spawner", &self.spawner.as_ref().map(|_| "SubagentSpawner"))
            .finish()
    }
}

impl ToolContext {
    /// Create an empty context (for non-streaming or test scenarios).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a context with session key only.
    pub fn with_session_key(session_key: SessionKey) -> Self {
        Self {
            session_key: Some(session_key),
            outbound_tx: None,
            spawner: None,
        }
    }

    /// Create a context with both session key and outbound channel.
    pub fn new(
        session_key: SessionKey,
        outbound_tx: tokio::sync::mpsc::Sender<OutboundMessage>,
    ) -> Self {
        Self {
            session_key: Some(session_key),
            outbound_tx: Some(outbound_tx),
            spawner: None,
        }
    }

    /// Create a context with spawner only.
    pub fn with_spawner(spawner: std::sync::Arc<dyn SubagentSpawner>) -> Self {
        Self {
            session_key: None,
            outbound_tx: None,
            spawner: Some(spawner),
        }
    }

    /// Create a context with session key and spawner.
    pub fn with_session_and_spawner(
        session_key: SessionKey,
        spawner: std::sync::Arc<dyn SubagentSpawner>,
    ) -> Self {
        Self {
            session_key: Some(session_key),
            outbound_tx: None,
            spawner: Some(spawner),
        }
    }

    /// Create a complete context with all fields set.
    pub fn complete(
        session_key: SessionKey,
        outbound_tx: tokio::sync::mpsc::Sender<OutboundMessage>,
        spawner: std::sync::Arc<dyn SubagentSpawner>,
    ) -> Self {
        Self {
            session_key: Some(session_key),
            outbound_tx: Some(outbound_tx),
            spawner: Some(spawner),
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
