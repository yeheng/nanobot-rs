//! Base trait for tools

use async_trait::async_trait;
use serde_json::Value;

/// Result type for tool execution
pub type ToolResult = Result<String, ToolError>;

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

    /// Execute the tool with given arguments
    async fn execute(&self, args: Value) -> ToolResult;
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
