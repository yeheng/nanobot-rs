//! Plugin manifest types
//!
//! This module defines the YAML manifest format for external plugins.
//! Plugins are declared via YAML manifests that describe their runtime
//! configuration, protocol, parameters, and required permissions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Manifest describing an external plugin.
///
/// The manifest is loaded from a YAML file and defines how the script
/// should be executed, what protocol it uses, what permissions it needs,
/// and what parameters it accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Tool name (must be unique across all tools)
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: String,

    /// Version string (default: "")
    #[serde(default)]
    pub version: String,

    /// Runtime configuration (command, args, working directory, etc.)
    pub runtime: RuntimeConfig,

    /// Communication protocol (default: Simple)
    #[serde(default)]
    pub protocol: PluginProtocol,

    /// JSON Schema defining the tool's parameters
    pub parameters: serde_json::Value,

    /// Required permissions (default: empty = deny all)
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

/// Communication protocol for plugins.
///
/// - Simple: stdin/stdout with newline-delimited JSON
/// - JsonRpc: JSON-RPC 2.0 over stdio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginProtocol {
    /// Simple stdin/stdout protocol (default)
    ///
    /// Input: JSON object via stdin
    /// Output: JSON object via stdout
    /// Errors: Non-zero exit code or stderr output
    #[default]
    Simple,

    /// JSON-RPC 2.0 over stdio
    ///
    /// Full JSON-RPC 2.0 protocol with method calls, notifications,
    /// and structured error responses.
    JsonRpc,
}

/// Runtime configuration for script execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Command to execute (e.g., "python", "node", "/path/to/script")
    pub command: String,

    /// Command arguments (default: [])
    #[serde(default)]
    pub args: Vec<String>,

    /// Working directory relative to manifest (default: ".")
    #[serde(default = "default_working_dir")]
    pub working_dir: String,

    /// Timeout in seconds (default: 120)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Environment variables to pass to the script (default: {})
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_working_dir() -> String {
    ".".to_string()
}

fn default_timeout() -> u64 {
    120
}

/// Permission grants access to specific Gasket capabilities.
///
/// Permissions map to JSON-RPC method names that will be made available
/// to the plugin. The default-deny policy means omitted permissions
/// result in no access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Permission to call LLM chat API
    LlmChat,

    /// Permission to search wiki pages
    WikiSearch,

    /// Permission to write wiki pages
    WikiWrite,

    /// Permission to run wiki decay
    WikiDecay,

    /// Permission to spawn subagents
    SubagentSpawn,
}

impl Permission {
    /// Get the JSON-RPC method name for this permission.
    ///
    /// Each permission maps to a specific RPC method that will be
    /// made available to the plugin.
    pub fn method_name(&self) -> &'static str {
        match self {
            Permission::LlmChat => "llm/chat",
            Permission::WikiSearch => "wiki/search",
            Permission::WikiWrite => "wiki/write",
            Permission::WikiDecay => "wiki/decay",
            Permission::SubagentSpawn => "subagent/spawn",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_manifest() {
        let yaml = r#"
name: "example_tool"
description: "An example tool"
version: "1.0.0"
runtime:
  command: "python"
  args: ["script.py"]
parameters:
  type: "object"
  properties:
    input:
      type: "string"
  required: ["input"]
"#;

        let manifest: PluginManifest =
            serde_yaml::from_str(yaml).expect("Failed to parse manifest");

        assert_eq!(manifest.name, "example_tool");
        assert_eq!(manifest.description, "An example tool");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.runtime.command, "python");
        assert_eq!(manifest.runtime.args, vec!["script.py"]);
        assert_eq!(manifest.runtime.working_dir, ".");
        assert_eq!(manifest.runtime.timeout_secs, 120);
        assert_eq!(manifest.protocol, PluginProtocol::Simple);
        assert!(manifest.runtime.env.is_empty());
        assert!(manifest.permissions.is_empty());
    }

    #[test]
    fn test_parse_jsonrpc_manifest_with_permissions() {
        let yaml = r#"
name: "advanced_tool"
description: "A tool with full configuration"
version: "2.0.0"
protocol: json_rpc
runtime:
  command: "node"
  args: ["index.js", "--verbose"]
  working_dir: "./scripts"
  timeout_secs: 300
  env:
    NODE_ENV: "production"
    API_KEY: "test-key"
parameters:
  type: "object"
  properties:
    query:
      type: "string"
  required: ["query"]
permissions:
  - llm_chat
  - wiki_search
  - wiki_write
"#;

        let manifest: PluginManifest =
            serde_yaml::from_str(yaml).expect("Failed to parse manifest");

        assert_eq!(manifest.name, "advanced_tool");
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.protocol, PluginProtocol::JsonRpc);
        assert_eq!(manifest.runtime.command, "node");
        assert_eq!(manifest.runtime.args, vec!["index.js", "--verbose"]);
        assert_eq!(manifest.runtime.working_dir, "./scripts");
        assert_eq!(manifest.runtime.timeout_secs, 300);
        assert_eq!(manifest.runtime.env.len(), 2);
        assert_eq!(
            manifest.runtime.env.get("NODE_ENV"),
            Some(&"production".to_string())
        );
        assert_eq!(
            manifest.runtime.env.get("API_KEY"),
            Some(&"test-key".to_string())
        );
        assert_eq!(manifest.permissions.len(), 3);
        assert!(manifest.permissions.contains(&Permission::LlmChat));
        assert!(manifest.permissions.contains(&Permission::WikiSearch));
        assert!(manifest.permissions.contains(&Permission::WikiWrite));
    }

    #[test]
    fn test_default_deny_no_permissions() {
        let yaml = r#"
name: "restricted_tool"
description: "A tool with no permissions"
runtime:
  command: "bash"
  args: ["script.sh"]
parameters:
  type: "object"
  properties: {}
"#;

        let manifest: PluginManifest =
            serde_yaml::from_str(yaml).expect("Failed to parse manifest");

        // Verify defaults for omitted fields
        assert_eq!(manifest.version, "");
        assert_eq!(manifest.protocol, PluginProtocol::Simple);
        assert_eq!(manifest.runtime.working_dir, ".");
        assert_eq!(manifest.runtime.timeout_secs, 120);
        assert!(manifest.runtime.env.is_empty());

        // Verify default-deny: no permissions specified = empty vector
        assert!(manifest.permissions.is_empty());
    }

    #[test]
    fn test_permission_serde_roundtrip() {
        let permissions = vec![
            Permission::LlmChat,
            Permission::WikiSearch,
            Permission::WikiWrite,
            Permission::WikiDecay,
            Permission::SubagentSpawn,
        ];

        // Test serialization
        let yaml = serde_yaml::to_string(&permissions).expect("Failed to serialize");
        let parsed: Vec<Permission> = serde_yaml::from_str(&yaml).expect("Failed to deserialize");

        // Verify all permissions survived roundtrip
        assert_eq!(parsed.len(), 5);
        assert!(parsed.contains(&Permission::LlmChat));
        assert!(parsed.contains(&Permission::WikiSearch));
        assert!(parsed.contains(&Permission::WikiWrite));
        assert!(parsed.contains(&Permission::WikiDecay));
        assert!(parsed.contains(&Permission::SubagentSpawn));
    }

    #[test]
    fn test_permission_method_names() {
        assert_eq!(Permission::LlmChat.method_name(), "llm/chat");
        assert_eq!(Permission::WikiSearch.method_name(), "wiki/search");
        assert_eq!(Permission::WikiWrite.method_name(), "wiki/write");
        assert_eq!(Permission::WikiDecay.method_name(), "wiki/decay");
        assert_eq!(Permission::SubagentSpawn.method_name(), "subagent/spawn");
    }
}
