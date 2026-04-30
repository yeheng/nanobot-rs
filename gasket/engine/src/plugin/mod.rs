//! Plugins module
//!
//! This module provides support for external plugins that can be
//! integrated into Gasket via YAML manifests. Plugins communicate via
//! either Simple (stdin/stdout JSON) or JSON-RPC 2.0 protocols.

pub mod dispatcher;
pub mod manifest;
pub mod rpc;
pub mod runner;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry, ToolResult};
pub use dispatcher::{
    build_dispatcher, DispatcherContext, EngineHandle, EngineResources, RpcDispatcher,
};
pub use manifest::{Permission, PluginManifest, PluginProtocol, RuntimeConfig};
pub use runner::{run_simple, JsonRpcDaemon, PluginError, PluginResult};

/// Plugin that implements the Tool trait for external scripts.
///
/// PluginTool wraps an external script with a YAML manifest and exposes
/// it as a native Gasket tool. It supports both Simple and JSON-RPC protocols.
///
/// JSON-RPC fields are `Option`al and filled by `with_engine_refs()`; the
/// protocol itself is determined by `manifest.protocol`, eliminating the
/// invalid-state matrix that required `unreachable!`.
#[derive(Clone)]
pub struct PluginTool {
    /// Parsed manifest describing the script
    manifest: PluginManifest,
    /// Directory containing the manifest (for resolving paths)
    manifest_dir: PathBuf,
    /// Dispatcher for JSON-RPC method routing (None for Simple plugins)
    dispatcher: Option<Arc<RpcDispatcher>>,
    /// Engine resources for JSON-RPC callbacks (None for Simple plugins)
    resources: Option<EngineResources>,
    /// Lazily spawned JSON-RPC daemon (None for Simple plugins)
    daemon: Option<Arc<tokio::sync::RwLock<Option<Arc<JsonRpcDaemon>>>>>,
}

impl PluginTool {
    /// Get the underlying manifest.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Create a new PluginTool from a manifest.
    ///
    /// Engine resources are injected at construction time. JSON-RPC plugins
    /// are fully initialized if `resources` is provided; otherwise they
    /// behave like Simple plugins (no callbacks).
    pub fn new(
        manifest: PluginManifest,
        manifest_dir: PathBuf,
        resources: Option<EngineResources>,
    ) -> Self {
        let (dispatcher, resources, daemon) =
            if manifest.protocol == PluginProtocol::JsonRpc && resources.is_some() {
                (
                    Some(Arc::new(build_dispatcher())),
                    resources,
                    Some(Arc::new(tokio::sync::RwLock::new(None))),
                )
            } else {
                (None, None, None)
            };
        Self {
            manifest,
            manifest_dir,
            dispatcher,
            resources,
            daemon,
        }
    }

    /// Build a dispatcher context from a tool context.
    ///
    /// Replaces the `session_key`, `outbound_tx`, `spawner` and `token_tracker`
    /// in the stored engine handle with the values from the current ToolContext.
    fn make_dispatch_ctx(&self, ctx: &ToolContext) -> Result<DispatcherContext, ToolError> {
        use dispatcher::EngineHandle;

        let resources = self.resources.as_ref().ok_or_else(|| {
            ToolError::ExecutionError(format!(
                "JSON-RPC plugin '{}' has not been initialized with engine resources. Ensure link_engine_refs() is called before execution.",
                self.manifest.name
            ))
        })?;

        Ok(DispatcherContext {
            engine: Arc::new(EngineHandle {
                session_key: ctx.session_key.clone(),
                outbound_tx: ctx.outbound_tx.clone(),
                spawner: ctx.spawner.clone(),
                token_tracker: ctx.token_tracker.clone(),
                tool_registry: resources.tool_registry.clone(),
                provider: resources.provider.clone(),
            }),
        })
    }

    /// Get or spawn the JSON-RPC daemon, handling idle expiration and deduplication.
    async fn get_or_spawn_daemon(
        &self,
        dispatch_ctx: &DispatcherContext,
    ) -> Result<Arc<JsonRpcDaemon>, ToolError> {
        let daemon = self.daemon.as_ref().ok_or_else(|| {
            ToolError::ExecutionError(
                "get_or_spawn_daemon called on non-JSON-RPC plugin".to_string(),
            )
        })?;

        let dispatcher = self.dispatcher.as_ref().ok_or_else(|| {
            ToolError::ExecutionError("JSON-RPC plugin missing dispatcher".to_string())
        })?;

        // Fast path: check existing daemon
        {
            let guard = daemon.read().await;
            if let Some(d) = guard.as_ref() {
                if !d.is_idle_expired() {
                    return Ok(d.clone());
                }
            }
        }

        // Slow path: acquire write lock and double-check
        let mut guard = daemon.write().await;
        if let Some(d) = guard.as_ref() {
            if !d.is_idle_expired() {
                return Ok(d.clone());
            }
        }

        let new_daemon = Arc::new(
            JsonRpcDaemon::spawn(
                &self.manifest,
                &self.manifest_dir,
                self.manifest.runtime.timeout_secs,
                &self.manifest.permissions,
                dispatcher,
                dispatch_ctx,
            )
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?,
        );
        *guard = Some(new_daemon.clone());
        Ok(new_daemon)
    }
}

#[async_trait]
impl Tool for PluginTool {
    /// Get the tool name from the manifest.
    fn name(&self) -> &str {
        &self.manifest.name
    }

    /// Get the tool description from the manifest.
    fn description(&self) -> &str {
        &self.manifest.description
    }

    /// Get the JSON schema for parameters from the manifest.
    fn parameters(&self) -> Value {
        self.manifest.parameters.clone()
    }

    /// Execute the plugin.
    ///
    /// # Protocol
    ///
    /// - **Simple mode**: One-shot execution with JSON input/output
    /// - **JSON-RPC mode**: Bidirectional communication with method calls
    ///
    /// # Returns
    ///
    /// - `Ok(String)` - JSON-encoded result with optional `_debug_stderr` field
    /// - `Err(ToolError)` - Spawn, timeout, exit, or protocol error
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Option<Box<dyn Tool>> {
        Some(Box::new(self.clone()))
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let start = std::time::Instant::now();

        let result = match self.manifest.protocol {
            PluginProtocol::Simple => {
                run_simple(
                    &self.manifest,
                    &self.manifest_dir,
                    &args,
                    self.manifest.runtime.timeout_secs,
                )
                .await
            }
            PluginProtocol::JsonRpc => {
                let dispatch_ctx = self.make_dispatch_ctx(ctx)?;
                let daemon = self.get_or_spawn_daemon(&dispatch_ctx).await?;
                daemon.call("initialize", Some(args)).await
            }
        };

        let duration = start.elapsed();

        match result {
            Ok(script_result) => {
                // Build output object
                let mut output = serde_json::Map::new();
                output.insert("result".to_string(), script_result.output);

                // Attach stderr as _debug_stderr field in JSON-RPC mode
                if self.manifest.protocol == PluginProtocol::JsonRpc
                    && !script_result.stderr.is_empty()
                {
                    output.insert("_debug_stderr".to_string(), script_result.stderr.into());
                }

                // Add duration metadata
                output.insert(
                    "_duration_ms".to_string(),
                    (duration.as_millis() as u64).into(),
                );

                // Serialize to JSON string
                serde_json::to_string(&Value::Object(output))
                    .map(|s| s.into())
                    .map_err(|e| {
                        ToolError::ExecutionError(format!("Failed to serialize result: {}", e))
                    })
            }
            Err(e) => Err(ToolError::ExecutionError(format!(
                "Plugin '{}' error: {}",
                self.manifest.name, e
            ))),
        }
    }
}

/// Discover and load all plugins in a directory.
///
/// Scans the directory for `*.yaml` files, parses them as manifests,
/// and creates PluginTool instances for valid manifests.
///
/// # Arguments
///
/// * `plugins_dir` - Directory to scan for manifest files
///
/// # Returns
///
/// * `Ok(Vec<PluginTool>)` - Vector of discovered plugins
/// * `Err(anyhow::Error)` - Directory read error or manifest parse error
pub fn discover_plugins_in_dir(plugins_dir: &Path) -> anyhow::Result<Vec<PluginTool>> {
    let mut tools = Vec::new();

    // Check if directory exists
    if !plugins_dir.exists() {
        info!(
            "Plugins directory does not exist: {:?}, skipping discovery",
            plugins_dir
        );
        return Ok(tools);
    }

    // Read directory entries
    let entries = std::fs::read_dir(plugins_dir).map_err(|e| {
        anyhow::anyhow!("Failed to read plugins directory {:?}: {}", plugins_dir, e)
    })?;

    // Process each YAML file
    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!("Failed to read directory entry in {:?}: {}", plugins_dir, e)
        })?;

        let path = entry.path();

        // Skip directories and non-YAML files
        if path.is_dir() {
            continue;
        }

        let ext = path.extension().and_then(|s| s.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        // Load manifest
        match load_manifest(&path) {
            Ok(manifest) => {
                info!("Discovered plugin '{}' from {:?}", manifest.name, path);
                let tool = PluginTool::new(manifest, plugins_dir.to_path_buf(), None);
                tools.push(tool);
            }
            Err(e) => {
                warn!("Failed to load manifest from {:?}: {}", path, e);
            }
        }
    }

    Ok(tools)
}

/// Discover and register all plugins in the Gasket plugins directory.
///
/// This is the main entry point for plugin discovery. It reads
/// `~/.gasket/plugins/` and registers all valid manifests with the
/// provided tool registry.
///
/// # Arguments
///
/// * `registry` - Tool registry to register discovered tools
/// * `engine` - Optional engine resources for JSON-RPC plugins.
///   If provided, JSON-RPC plugins are pre-configured with engine resources.
///   If `None`, they are still registered but require `link_engine_refs()`
///   before they can be executed.
///
/// # Returns
///
/// * `Ok(())` - Discovery completed successfully
/// * `Err(anyhow::Error)` - Directory read or manifest parse error
///
/// # Note
///
/// If the plugins directory does not exist, this function returns `Ok(())`
/// without error. Missing directories are treated as empty tool sets.
pub fn discover_plugins(
    registry: &mut ToolRegistry,
    engine: Option<EngineResources>,
) -> anyhow::Result<()> {
    // Resolve plugins directory: ~/.gasket/plugins/
    let plugins_dir = dirs::home_dir()
        .map(|home| home.join(".gasket/plugins"))
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve home directory"))?;

    // Discover tools
    let tools = discover_plugins_in_dir(&plugins_dir)?;

    // Register each tool — engine resources are injected at construction time.
    for tool in &tools {
        let tool = PluginTool::new(
            tool.manifest.clone(),
            tool.manifest_dir.clone(),
            engine.clone(),
        );
        registry.register(Box::new(tool));
    }

    info!(
        "Discovered and registered {} plugins from {:?}",
        tools.len(),
        plugins_dir
    );

    Ok(())
}

/// Load a script manifest from a YAML file.
///
/// # Arguments
///
/// * `path` - Path to the manifest file
///
/// # Returns
///
/// * `Ok(PluginManifest)` - Parsed manifest
/// * `Err(anyhow::Error)` - File read or YAML parse error
fn load_manifest(path: &Path) -> anyhow::Result<PluginManifest> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read manifest file {:?}: {}", path, e))?;

    let manifest: PluginManifest = serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse manifest YAML from {:?}: {}", path, e))?;

    // Validate manifest
    if manifest.name.is_empty() {
        return Err(anyhow::anyhow!("Manifest from {:?} has empty name", path));
    }

    if manifest.description.is_empty() {
        return Err(anyhow::anyhow!(
            "Manifest from {:?} has empty description",
            path
        ));
    }

    if manifest.runtime.command.is_empty() {
        return Err(anyhow::anyhow!(
            "Manifest from {:?} has empty command",
            path
        ));
    }

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Create a minimal manifest for testing.
    fn test_manifest(command: &str) -> (PluginManifest, TempDir) {
        let dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: RuntimeConfig {
                command: command.to_string(),
                args: vec![],
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: PluginProtocol::Simple,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            permissions: vec![],
        };
        (manifest, dir)
    }

    #[test]
    fn test_script_tool_new() {
        let (manifest, dir) = test_manifest("cat");
        let tool = PluginTool::new(manifest, dir.path().to_path_buf(), None);

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "Test tool");
        assert_eq!(tool.parameters()["type"], "object");
    }

    #[tokio::test]
    async fn test_simple_tool_execute() {
        let (manifest, dir) = test_manifest("cat");
        let tool = PluginTool::new(manifest, dir.path().to_path_buf(), None);

        let args = json!({"hello": "world", "number": 42});
        let ctx = ToolContext::default();

        let result = tool.execute(args, &ctx).await;

        assert!(result.is_ok());
        let output_str = result.unwrap();
        let output: Value = serde_json::from_str(&output_str.content).unwrap();

        // Verify result field contains echoed input
        assert_eq!(output["result"]["hello"], "world");
        assert_eq!(output["result"]["number"], 42);

        // Verify metadata fields
        assert!(output["_duration_ms"].is_number());
    }

    #[test]
    fn test_discover_plugins_no_dir() {
        // Use a nonexistent directory path
        let nonexistent = PathBuf::from("/tmp/nonexistent_gasket_scripts_xyz123");

        let tools = discover_plugins_in_dir(&nonexistent).unwrap();

        // Should return empty vec without error
        assert_eq!(tools.len(), 0);
    }

    #[test]
    fn test_load_manifest_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        let manifest_path = dir.path().join("invalid.yaml");

        // Write invalid YAML
        std::fs::write(&manifest_path, "invalid: yaml: content: [").unwrap();

        let result = load_manifest(&manifest_path);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse"));
    }

    #[test]
    fn test_load_manifest_missing_command() {
        let dir = TempDir::new().unwrap();
        let manifest_path = dir.path().join("no_command.yaml");

        let yaml = r#"
name: "test_tool"
description: "Tool with no command"
runtime:
  command: ""
parameters:
  type: "object"
  properties: {}
"#;

        std::fs::write(&manifest_path, yaml).unwrap();

        let result = load_manifest(&manifest_path);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty command"));
    }

    #[test]
    fn test_script_tool_make_dispatch_ctx() {
        use gasket_providers::LlmProvider;
        use gasket_types::{
            token_tracker::TokenTracker, ChannelType, OutboundMessage, SessionKey, SubagentResult,
            SubagentSpawner,
        };
        use std::sync::Arc;

        struct MockSpawner;
        #[async_trait::async_trait]
        impl SubagentSpawner for MockSpawner {
            async fn spawn(
                &self,
                _task: String,
                _model_id: Option<String>,
            ) -> Result<SubagentResult, Box<dyn std::error::Error + Send>> {
                Ok(SubagentResult {
                    id: "mock".to_string(),
                    task: "mock".to_string(),
                    response: gasket_types::SubagentResponse {
                        content: "mock".to_string(),
                        reasoning_content: None,
                        tools_used: vec![],
                        model: None,
                        token_usage: None,
                        cost: 0.0,
                    },
                    model: None,
                })
            }
        }

        struct MockProvider;
        #[async_trait::async_trait]
        impl LlmProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            fn default_model(&self) -> &str {
                "mock-model"
            }
            async fn chat(
                &self,
                _request: gasket_providers::ChatRequest,
            ) -> Result<gasket_providers::ChatResponse, gasket_providers::ProviderError>
            {
                Ok(gasket_providers::ChatResponse {
                    content: Some("Test response".to_string()),
                    tool_calls: vec![],
                    reasoning_content: None,
                    usage: Some(gasket_providers::Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                    }),
                finish_reason: None,
                })
            }
        }

        let dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: RuntimeConfig {
                command: "cat".to_string(),
                args: vec![],
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: PluginProtocol::JsonRpc,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            permissions: vec![],
        };

        let tool = PluginTool::new(
            manifest,
            dir.path().to_path_buf(),
            Some(EngineResources {
                tool_registry: Arc::new(ToolRegistry::new()),
                provider: Arc::new(MockProvider),
            }),
        );

        let (tx, _rx) = tokio::sync::mpsc::channel::<OutboundMessage>(1);
        let ctx = ToolContext::default()
            .session_key(SessionKey::new(ChannelType::Telegram, "test-chat"))
            .outbound_tx(tx)
            .spawner(Arc::new(MockSpawner))
            .token_tracker(Arc::new(TokenTracker::unlimited("USD")));

        // Just verify it doesn't crash when all required refs are present
        let _dispatch_ctx = tool.make_dispatch_ctx(&ctx);
    }
}
