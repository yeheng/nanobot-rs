//! Script tools module
//!
//! This module provides support for external script tools that can be
//! integrated into Gasket via YAML manifests. Scripts communicate via
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

use super::{Tool, ToolContext, ToolError, ToolRegistry, ToolResult};
use gasket_providers::LlmProvider;

pub use dispatcher::{build_dispatcher, DispatcherContext, EngineHandle, RpcDispatcher};
pub use manifest::{Permission, RuntimeConfig, ScriptManifest, ScriptProtocol};
pub use runner::{run_jsonrpc, run_simple, JsonRpcDaemon, ScriptError, ScriptResult};

/// Script tool that implements the Tool trait for external scripts.
///
/// ScriptTool wraps an external script with a YAML manifest and exposes
/// it as a native Gasket tool. It supports both Simple and JSON-RPC protocols.
#[derive(Clone)]
pub struct ScriptTool {
    /// Parsed manifest describing the script
    manifest: ScriptManifest,
    /// Directory containing the manifest (for resolving paths)
    manifest_dir: PathBuf,
    /// RPC dispatcher for handling method calls (JSON-RPC mode only)
    dispatcher: Arc<RpcDispatcher>,
    /// Tool registry for delegating to other tools (injected post-construction)
    tool_registry: Option<Arc<ToolRegistry>>,
    /// LLM provider for chat completions (injected post-construction)
    provider: Option<Arc<dyn LlmProvider>>,
    /// Persistent JSON-RPC daemon (lazy-initialized, JSON-RPC mode only)
    daemon: Arc<tokio::sync::RwLock<Option<Arc<JsonRpcDaemon>>>>,
}

impl ScriptTool {
    /// Create a new ScriptTool from a manifest.
    ///
    /// # Arguments
    ///
    /// * `manifest` - Parsed script manifest
    /// * `manifest_dir` - Directory containing the manifest file
    ///
    /// # Returns
    ///
    /// A new ScriptTool instance with an empty dispatcher.
    /// Use `with_engine_refs()` to inject engine capabilities.
    pub fn new(manifest: ScriptManifest, manifest_dir: PathBuf) -> Self {
        let dispatcher = Arc::new(build_dispatcher());
        Self {
            manifest,
            manifest_dir,
            dispatcher,
            tool_registry: None,
            provider: None,
            daemon: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Inject engine references for JSON-RPC mode.
    ///
    /// This is a separate method to avoid circular dependencies during
    /// construction. The tool registry and provider are needed for
    /// JSON-RPC handlers but aren't available when the manifest is loaded.
    ///
    /// # Arguments
    ///
    /// * `registry` - Tool registry for delegating to other tools
    /// * `provider` - LLM provider for chat completions
    ///
    /// # Returns
    ///
    /// Self for method chaining
    pub fn with_engine_refs(
        mut self,
        registry: Arc<ToolRegistry>,
        provider: Arc<dyn LlmProvider>,
    ) -> Self {
        self.tool_registry = Some(registry);
        self.provider = Some(provider);
        self
    }

    /// Build a dispatcher context from a tool context.
    ///
    /// Extracts the relevant fields from ToolContext and constructs
    /// a DispatcherContext for JSON-RPC handler execution.
    fn make_dispatch_ctx(&self, ctx: &ToolContext) -> DispatcherContext {
        use dispatcher::EngineHandle;

        // Get required engine components - all must be present for JSON-RPC mode
        let tool_registry = self
            .tool_registry
            .clone()
            .expect("ScriptTool missing tool_registry - call with_engine_refs()");
        let provider = self
            .provider
            .clone()
            .expect("ScriptTool missing provider - call with_engine_refs()");

        DispatcherContext {
            engine: Arc::new(EngineHandle {
                session_key: ctx
                    .session_key
                    .clone()
                    .expect("session_key required for JSON-RPC mode"),
                outbound_tx: ctx
                    .outbound_tx
                    .clone()
                    .expect("outbound_tx required for JSON-RPC mode"),
                spawner: ctx
                    .spawner
                    .clone()
                    .expect("spawner required for JSON-RPC mode"),
                token_tracker: ctx
                    .token_tracker
                    .clone()
                    .expect("token_tracker required for JSON-RPC mode"),
                tool_registry,
                provider,
            }),
        }
    }
}

#[async_trait]
impl Tool for ScriptTool {
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

    /// Execute the script tool.
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
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let start = std::time::Instant::now();

        let result = match self.manifest.protocol {
            ScriptProtocol::Simple => {
                run_simple(
                    &self.manifest,
                    &self.manifest_dir,
                    &args,
                    self.manifest.runtime.timeout_secs,
                )
                .await
            }
            ScriptProtocol::JsonRpc => {
                let dispatch_ctx = self.make_dispatch_ctx(ctx);

                // Fast path: read-check daemon
                let daemon = {
                    let guard = self.daemon.read().await;
                    guard.as_ref().and_then(|d| {
                        if d.is_idle_expired() {
                            None
                        } else {
                            Some(d.clone())
                        }
                    })
                };

                let daemon = match daemon {
                    Some(d) => d,
                    None => {
                        let mut guard = self.daemon.write().await;
                        if let Some(d) = guard.as_ref() {
                            if !d.is_idle_expired() {
                                d.clone()
                            } else {
                                let new_daemon = Arc::new(
                                    JsonRpcDaemon::spawn(
                                        &self.manifest,
                                        &self.manifest_dir,
                                        self.manifest.runtime.timeout_secs,
                                        &self.manifest.permissions,
                                        &self.dispatcher,
                                        &dispatch_ctx,
                                    )
                                    .await
                                    .map_err(|e| ToolError::ExecutionError(e.to_string()))?,
                                );
                                *guard = Some(new_daemon.clone());
                                new_daemon
                            }
                        } else {
                            let new_daemon = Arc::new(
                                JsonRpcDaemon::spawn(
                                    &self.manifest,
                                    &self.manifest_dir,
                                    self.manifest.runtime.timeout_secs,
                                    &self.manifest.permissions,
                                    &self.dispatcher,
                                    &dispatch_ctx,
                                )
                                .await
                                .map_err(|e| ToolError::ExecutionError(e.to_string()))?,
                            );
                            *guard = Some(new_daemon.clone());
                            new_daemon
                        }
                    }
                };

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
                if self.manifest.protocol == ScriptProtocol::JsonRpc
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
                serde_json::to_string(&Value::Object(output)).map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to serialize result: {}", e))
                })
            }
            Err(e) => match e {
                ScriptError::SpawnFailed(msg) => Err(ToolError::ExecutionError(format!(
                    "Failed to spawn script '{}': {}",
                    self.manifest.name, msg
                ))),
                ScriptError::Timeout(secs) => Err(ToolError::ExecutionError(format!(
                    "Script '{}' timed out after {}s",
                    self.manifest.name, secs
                ))),
                ScriptError::NonZeroExit(code) => Err(ToolError::ExecutionError(format!(
                    "Script '{}' exited with non-zero code: {:?}",
                    self.manifest.name, code
                ))),
                ScriptError::InvalidOutput(msg) => Err(ToolError::ExecutionError(format!(
                    "Script '{}' returned invalid output: {}",
                    self.manifest.name, msg
                ))),
                ScriptError::Io(msg) => Err(ToolError::ExecutionError(format!(
                    "Script '{}' I/O error: {}",
                    self.manifest.name, msg
                ))),
            },
        }
    }
}

/// Discover and load all script tools in a directory.
///
/// Scans the directory for `*.yaml` files, parses them as manifests,
/// and creates ScriptTool instances for valid manifests.
///
/// # Arguments
///
/// * `scripts_dir` - Directory to scan for manifest files
///
/// # Returns
///
/// * `Ok(Vec<ScriptTool>)` - Vector of discovered script tools
/// * `Err(anyhow::Error)` - Directory read error or manifest parse error
pub fn discover_scripts_in_dir(scripts_dir: &Path) -> anyhow::Result<Vec<ScriptTool>> {
    let mut tools = Vec::new();

    // Check if directory exists
    if !scripts_dir.exists() {
        info!(
            "Scripts directory does not exist: {:?}, skipping discovery",
            scripts_dir
        );
        return Ok(tools);
    }

    // Read directory entries
    let entries = std::fs::read_dir(scripts_dir).map_err(|e| {
        anyhow::anyhow!("Failed to read scripts directory {:?}: {}", scripts_dir, e)
    })?;

    // Process each YAML file
    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!("Failed to read directory entry in {:?}: {}", scripts_dir, e)
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
                info!("Discovered script tool '{}' from {:?}", manifest.name, path);
                let tool = ScriptTool::new(manifest, scripts_dir.to_path_buf());
                tools.push(tool);
            }
            Err(e) => {
                warn!("Failed to load manifest from {:?}: {}", path, e);
            }
        }
    }

    Ok(tools)
}

/// Discover and register all script tools in the Gasket scripts directory.
///
/// This is the main entry point for script tool discovery. It reads
/// `~/.gasket/scripts/` and registers all valid manifests with the
/// provided tool registry.
///
/// # Arguments
///
/// * `registry` - Tool registry to register discovered tools
/// * `engine_registry` - Optional engine tool registry for JSON-RPC handlers
/// * `provider` - Optional LLM provider for JSON-RPC handlers
///
/// # Returns
///
/// * `Ok(())` - Discovery completed successfully
/// * `Err(anyhow::Error)` - Directory read or manifest parse error
///
/// # Note
///
/// If the scripts directory does not exist, this function returns `Ok(())`
/// without error. Missing directories are treated as empty tool sets.
pub fn discover_scripts(
    registry: &mut ToolRegistry,
    engine_registry: Option<Arc<ToolRegistry>>,
    provider: Option<Arc<dyn LlmProvider>>,
) -> anyhow::Result<()> {
    // Resolve scripts directory: ~/.gasket/scripts/
    let scripts_dir = dirs::home_dir()
        .map(|home| home.join(".gasket/scripts"))
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve home directory"))?;

    // Discover tools
    let tools = discover_scripts_in_dir(&scripts_dir)?;

    // Register each tool
    for tool in &tools {
        let mut tool = tool.clone();

        // Inject engine references if both are provided
        if let (Some(reg), Some(prov)) = (engine_registry.clone(), provider.clone()) {
            tool = tool.with_engine_refs(reg, prov);
        }

        // Register with registry
        registry.register(Box::new(tool));
    }

    info!(
        "Discovered and registered {} script tools from {:?}",
        tools.len(),
        scripts_dir
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
/// * `Ok(ScriptManifest)` - Parsed manifest
/// * `Err(anyhow::Error)` - File read or YAML parse error
fn load_manifest(path: &Path) -> anyhow::Result<ScriptManifest> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read manifest file {:?}: {}", path, e))?;

    let manifest: ScriptManifest = serde_yaml::from_str(&content)
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
    fn test_manifest(command: &str) -> (ScriptManifest, TempDir) {
        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
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
            protocol: ScriptProtocol::Simple,
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
        let tool = ScriptTool::new(manifest, dir.path().to_path_buf());

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "Test tool");
        assert_eq!(tool.parameters()["type"], "object");
    }

    #[tokio::test]
    async fn test_simple_tool_execute() {
        let (manifest, dir) = test_manifest("cat");
        let tool = ScriptTool::new(manifest, dir.path().to_path_buf());

        let args = json!({"hello": "world", "number": 42});
        let ctx = ToolContext::default();

        let result = tool.execute(args, &ctx).await;

        assert!(result.is_ok());
        let output_str = result.unwrap();
        let output: Value = serde_json::from_str(&output_str).unwrap();

        // Verify result field contains echoed input
        assert_eq!(output["result"]["hello"], "world");
        assert_eq!(output["result"]["number"], 42);

        // Verify metadata fields
        assert!(output["_duration_ms"].is_number());
    }

    #[test]
    fn test_discover_scripts_no_dir() {
        // Use a nonexistent directory path
        let nonexistent = PathBuf::from("/tmp/nonexistent_gasket_scripts_xyz123");

        let tools = discover_scripts_in_dir(&nonexistent).unwrap();

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
                })
            }
        }

        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
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
            protocol: ScriptProtocol::JsonRpc,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            permissions: vec![],
        };

        let tool = ScriptTool::new(manifest, dir.path().to_path_buf())
            .with_engine_refs(Arc::new(ToolRegistry::new()), Arc::new(MockProvider));

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
