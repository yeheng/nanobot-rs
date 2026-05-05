//! Bridge: expose registered tools as slash commands.
//!
//! Any tool in `ToolRegistry` (including plugins) becomes callable as
//! `/toolname [args]` without going through the LLM.

use std::sync::Arc;

use futures_util::FutureExt;
use gasket_command::dispatcher::DispatcherBuilder;
use gasket_command::types::{Command, CommandKind, CommandResult};
use gasket_engine::tools::{Tool, ToolContext, ToolRegistry};
use gasket_types::SessionKey;

/// Parse raw command-line args into JSON for tool invocation.
///
/// - Empty args          → `{}`
/// - JSON (`{...}`)      → parsed directly
/// - Plain text          → `{ <first_string_prop>: text }` or `{ "input": text }`
pub fn parse_tool_args(args: &str, parameters: &serde_json::Value) -> serde_json::Value {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return serde_json::json!({});
    }

    // Attempt JSON parse first
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(val) = serde_json::from_str(trimmed) {
            return val;
        }
    }

    // Fallback: map plain text to the first string property in the schema
    if let Some(props) = parameters.get("properties").and_then(|p| p.as_object()) {
        for (key, schema) in props {
            if schema.get("type").and_then(|t| t.as_str()) == Some("string") {
                return serde_json::json!({ key: trimmed });
            }
        }
    }

    // Ultimate fallback
    serde_json::json!({ "input": trimmed })
}

/// Turn a single `Tool` into a `Command` that the dispatcher understands.
fn tool_to_command(tool: &dyn Tool, tool_registry: Arc<ToolRegistry>) -> Command {
    let name = tool.name().to_string();
    let description = tool.description().to_string();
    let parameters = tool.parameters();

    Command {
        name: name.clone(),
        description,
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(
            move |args: &str, _host: &dyn gasket_command::CommandHost, _key: &SessionKey| {
                let tool_registry = tool_registry.clone();
                let name = name.clone();
                let parameters = parameters.clone();
                let parsed = parse_tool_args(args, &parameters);

                async move {
                    let ctx = ToolContext::default();
                    match tool_registry.execute(&name, parsed, &ctx).await {
                        Ok(output) => CommandResult::Print(output),
                        Err(e) => CommandResult::Error(format!("Tool error: {e}")),
                    }
                }
                .boxed()
            },
        )),
    }
}

/// Register every tool in `tool_registry` as a slash command on the builder.
pub fn register_tool_commands(
    mut builder: DispatcherBuilder,
    tool_registry: Arc<ToolRegistry>,
) -> DispatcherBuilder {
    for name in tool_registry.list() {
        if let Some(tool) = tool_registry.get(name) {
            let cmd = tool_to_command(tool, tool_registry.clone());
            builder = builder.register_builtin(cmd);
        }
    }
    builder
}
