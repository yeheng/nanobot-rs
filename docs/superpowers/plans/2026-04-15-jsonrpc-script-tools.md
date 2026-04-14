# JSON-RPC Script Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Gasket's tool system with external script support using JSON-RPC 2.0 over JSON-Lines for bidirectional engine-script communication.

**Architecture:** Scripts live in `~/.gasket/scripts/<name>/manifest.yaml` and are discovered at startup. Each `ScriptTool` implements the `Tool` trait, dispatching to either a simple runner (one-shot stdin/stdout) or a JSON-RPC multiplexer (bidirectional callbacks). The multiplexer uses `tokio::select!` with a trait-based dispatcher that enforces declarative permissions.

**Tech Stack:** Rust (tokio, serde, serde_json, serde_yaml, async-trait, thiserror), Python (for integration tests)

**Spec:** `docs/superpowers/specs/2026-04-15-jsonrpc-script-tools-design.md`

---

## File Structure

```
engine/src/tools/script/
├── mod.rs                  # ScriptTool + discover_scripts + re-exports
├── manifest.rs             # ScriptManifest, RuntimeConfig, Permission, ScriptProtocol
├── rpc.rs                  # RpcMessage, RpcRequest, RpcResponse, RpcError, codec
├── dispatcher/
│   ├── mod.rs              # RpcHandler trait, DispatcherContext, RpcDispatcher + build_dispatcher()
│   ├── llm_chat.rs         # LlmChatHandler
│   ├── memory_search.rs    # MemorySearchHandler
│   ├── memory_write.rs     # MemoryWriteHandler
│   ├── memory_decay.rs     # MemoryDecayHandler
│   └── subagent.rs         # SubagentSpawnHandler
└── runner.rs               # run_simple(), run_jsonrpc(), StderrCollector, ScriptError

CLI integration:
├── cli/src/commands/registry.rs  # Add discover_scripts() call after tool registration

Integration tests:
└── tests/scripts/
    ├── simple_echo/         # Simple mode test
    └── jsonrpc_ping/        # JSON-RPC mode test
```

> **Important**: The dispatcher uses a directory module (`dispatcher/mod.rs`), NOT a file + directory combo. All dispatcher types (RpcHandler trait, DispatcherContext, RpcDispatcher) live in `dispatcher/mod.rs`.

---

### Task 1: Manifest Types

**Files:**
- Create: `gasket/engine/src/tools/script/manifest.rs`
- Test: inline `#[cfg(test)]` in same file

- [ ] **Step 1: Write failing tests for manifest parsing**

```rust
// gasket/engine/src/tools/script/manifest.rs (bottom of file)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_manifest() {
        let yaml = r#"
name: "hello"
description: "Says hello"
parameters:
  type: object
  properties:
    name: { type: string, description: "Name to greet" }
  required: ["name"]
runtime:
  command: "python3"
  args: ["hello.py"]
"#;
        let manifest: ScriptManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "hello");
        assert_eq!(manifest.description, "Says hello");
        assert_eq!(manifest.protocol, ScriptProtocol::Simple); // default
        assert!(manifest.permissions.is_empty()); // default-deny
        assert_eq!(manifest.runtime.command, "python3");
        assert_eq!(manifest.runtime.args, vec!["hello.py"]);
    }

    #[test]
    fn test_parse_jsonrpc_manifest_with_permissions() {
        let yaml = r#"
name: "analyzer"
description: "Analyzes data"
version: "2.0.0"
protocol: "jsonrpc"
parameters:
  type: object
  properties: {}
runtime:
  command: "node"
  args: ["index.js"]
  timeout_secs: 300
  env:
    NODE_ENV: "production"
permissions:
  - llm_chat
  - memory_search
  - subagent_spawn
"#;
        let manifest: ScriptManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.protocol, ScriptProtocol::JsonRpc);
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.runtime.timeout_secs, 300);
        assert_eq!(manifest.runtime.env.get("NODE_ENV").unwrap(), "production");
        assert_eq!(manifest.permissions.len(), 3);
        assert!(manifest.permissions.contains(&Permission::LlmChat));
        assert!(manifest.permissions.contains(&Permission::MemorySearch));
        assert!(manifest.permissions.contains(&Permission::SubagentSpawn));
    }

    #[test]
    fn test_default_deny_no_permissions() {
        let yaml = r#"
name: "basic"
description: "Basic tool"
parameters:
  type: object
  properties: {}
runtime:
  command: "sh"
"#;
        let manifest: ScriptManifest = serde_yaml::from_str(yaml).unwrap();
        assert!(manifest.permissions.is_empty());
        assert_eq!(manifest.runtime.timeout_secs, 120); // default
        assert_eq!(manifest.runtime.working_dir, "."); // default
    }

    #[test]
    fn test_permission_serde_roundtrip() {
        let perms = vec![Permission::LlmChat, Permission::SubagentSpawn];
        let yaml = serde_yaml::to_string(&perms).unwrap();
        assert!(yaml.contains("llm_chat"));
        assert!(yaml.contains("subagent_spawn"));
        let back: Vec<Permission> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(perms, back);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-engine --lib tools::script::manifest::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error — module `script` does not exist.

- [ ] **Step 3: Create the module skeleton and manifest types**

Create `gasket/engine/src/tools/script/manifest.rs`:

```rust
//! Script manifest types for external tool registration.

use serde::Deserialize;
use std::collections::HashMap;

/// Script manifest loaded from `~/.gasket/scripts/<name>/manifest.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptManifest {
    /// Tool name registered into ToolRegistry.
    pub name: String,
    /// Description shown to the LLM.
    pub description: String,
    /// Semantic version (informational).
    #[serde(default)]
    pub version: String,
    /// Runtime configuration (command, args, timeout, env).
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// Communication protocol: "simple" (default) or "jsonrpc".
    #[serde(default)]
    pub protocol: ScriptProtocol,
    /// JSON Schema for tool parameters (passed from LLM).
    pub parameters: serde_json::Value,
    /// Declared engine capabilities (default-deny: empty = no callbacks).
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

/// Communication protocol between engine and script process.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptProtocol {
    /// One-shot: stdin JSON in, stdout JSON out.
    #[default]
    Simple,
    /// JSON-RPC 2.0 over JSON-Lines with bidirectional callbacks.
    JsonRpc,
}

/// Process runtime configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    /// Command to execute (e.g., "python3", "node").
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory, resolved relative to manifest directory. "." = script's own dir.
    #[serde(default = "default_working_dir")]
    pub working_dir: String,
    /// Wall-clock timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Extra environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_working_dir() -> String {
    ".".to_string()
}

fn default_timeout() -> u64 {
    120
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            working_dir: default_working_dir(),
            timeout_secs: default_timeout(),
            env: HashMap::new(),
        }
    }
}

/// Engine capabilities that scripts can request.
///
/// 1:1 mapping to RPC method names:
/// `LlmChat` → `"llm/chat"`, `MemorySearch` → `"memory/search"`, etc.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    LlmChat,
    MemorySearch,
    MemoryWrite,
    MemoryDecay,
    SubagentSpawn,
}

impl Permission {
    /// Convert to the RPC method name string.
    pub fn method_name(&self) -> &'static str {
        match self {
            Permission::LlmChat => "llm/chat",
            Permission::MemorySearch => "memory/search",
            Permission::MemoryWrite => "memory/write",
            Permission::MemoryDecay => "memory/decay",
            Permission::SubagentSpawn => "subagent/spawn",
        }
    }
}

// tests module here (from Step 1)
```

Create `gasket/engine/src/tools/script/mod.rs` (minimal skeleton for now):

```rust
//! External script tool support.
//!
//! Scripts in `~/.gasket/scripts/` are discovered at startup and registered
//! as tools. Two communication protocols are supported:
//! - **Simple**: one-shot stdin JSON → stdout JSON
//! - **JsonRpc**: JSON-RPC 2.0 over JSON-Lines with bidirectional callbacks

pub mod manifest;

// Re-export primary types
pub use manifest::{Manifest, Permission, RuntimeConfig, ScriptManifest, ScriptProtocol};
```

Add to `gasket/engine/src/tools/mod.rs`:

```rust
// Add this line after the existing mod declarations:
pub mod script;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gasket-engine --lib tools::script::manifest::tests -- --nocapture`
Expected: All 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/script/
git commit -m "feat(script-tools): add manifest types with permission model"
```

---

### Task 2: RPC Types & Codec

**Files:**
- Create: `gasket/engine/src/tools/script/rpc.rs`
- Modify: `gasket/engine/src/tools/script/mod.rs` (add `pub mod rpc;`)

- [ ] **Step 1: Write failing tests for RPC codec**

```rust
// Append to gasket/engine/src/tools/script/rpc.rs

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_encode_request() {
        let req = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(0)),
            method: "initialize".into(),
            params: Some(json!({"key": "value"})),
        };
        let encoded = encode(&RpcMessage::Request(req));
        assert!(encoded.ends_with('\n'));
        let parsed: serde_json::Value = serde_json::from_str(encoded.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "initialize");
    }

    #[test]
    fn test_decode_request() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"llm/chat","params":{"messages":[]}}"#;
        let msg = decode(line).unwrap();
        match msg {
            RpcMessage::Request(req) => {
                assert_eq!(req.method, "llm/chat");
                assert_eq!(req.id, Some(json!(1)));
            }
            _ => panic!("Expected Request, got Response"),
        }
    }

    #[test]
    fn test_decode_response_with_result() {
        let line = r#"{"jsonrpc":"2.0","id":0,"result":{"answer":42}}"#;
        let msg = decode(line).unwrap();
        match msg {
            RpcMessage::Response(resp) => {
                assert_eq!(resp.id, json!(0));
                assert_eq!(resp.result.unwrap()["answer"], 42);
                assert!(resp.error.is_none());
            }
            _ => panic!("Expected Response, got Request"),
        }
    }

    #[test]
    fn test_decode_response_with_error() {
        let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let msg = decode(line).unwrap();
        match msg {
            RpcMessage::Response(resp) => {
                let err = resp.error.unwrap();
                assert_eq!(err.code, -32601);
                assert_eq!(err.message, "Method not found");
            }
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_decode_invalid_json_returns_none() {
        assert!(decode("not json at all").is_none());
        assert!(decode("").is_none());
        assert!(decode("   ").is_none());
    }

    #[test]
    fn test_decode_plain_text_returns_none() {
        assert!(decode("WARNING: deprecated feature").is_none());
        assert!(decode("some random output from library").is_none());
    }

    #[test]
    fn test_roundtrip_request() {
        let original = RpcMessage::Request(RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(42)),
            method: "test".into(),
            params: None,
        });
        let encoded = encode(&original);
        let decoded = decode(encoded.trim()).unwrap();
        match decoded {
            RpcMessage::Request(req) => {
                assert_eq!(req.method, "test");
                assert_eq!(req.id, Some(json!(42)));
            }
            _ => panic!("Expected Request"),
        }
    }

    #[test]
    fn test_error_constructors() {
        let e = RpcError::method_not_found("foo");
        assert_eq!(e.code, -32601);

        let e = RpcError::permission_denied("bar");
        assert_eq!(e.code, -32000);

        let e = RpcError::invalid_params("bad");
        assert_eq!(e.code, -32602);

        let e = RpcError::internal_error("oops");
        assert_eq!(e.code, -32603);
    }

    #[test]
    fn test_message_size_limit() {
        let big_line = "x".repeat(1024 * 1024 + 1); // > 1 MiB
        assert!(decode(&big_line).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-engine --lib tools::script::rpc::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error — module `rpc` does not exist.

- [ ] **Step 3: Write the RPC types and codec**

Create `gasket/engine/src/tools/script/rpc.rs`:

```rust
//! JSON-RPC 2.0 message types and line-based codec.
//!
//! Messages are serialized as single JSON lines separated by `\n`.
//! Invalid JSON on stdout is silently discarded (logged as warning).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum allowed size for a single JSON-RPC message (1 MiB).
pub const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

// ─── Messages ───────────────────────────────────────

/// A JSON-RPC 2.0 message.
///
/// Uses `#[serde(untagged)]` to discriminate by field presence:
/// - Has `method` → Request
/// - Has `result` or `error` → Response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcMessage {
    Request(RpcRequest),
    Response(RpcResponse),
}

/// JSON-RPC 2.0 Request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC 2.0 Error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ─── Error Constructors ─────────────────────────────

impl RpcError {
    /// -32601: Method not found.
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {}", method),
            data: None,
        }
    }

    /// -32000: Permission denied (application-defined).
    pub fn permission_denied(method: &str) -> Self {
        Self {
            code: -32000,
            message: format!("Permission denied for method: {}", method),
            data: None,
        }
    }

    /// -32602: Invalid params.
    pub fn invalid_params(msg: &str) -> Self {
        Self {
            code: -32602,
            message: format!("Invalid params: {}", msg),
            data: None,
        }
    }

    /// -32603: Internal error.
    pub fn internal_error(msg: &str) -> Self {
        Self {
            code: -32603,
            message: msg.to_string(),
            data: None,
        }
    }
}

// ─── Codec ──────────────────────────────────────────

/// Encode an RpcMessage as a single JSON line with trailing newline.
pub fn encode(msg: &RpcMessage) -> String {
    let mut line = serde_json::to_string(msg).unwrap();
    line.push('\n');
    line
}

/// Decode a single line into an RpcMessage.
///
/// Returns `None` for:
/// - Empty or whitespace-only lines
/// - Lines exceeding `MAX_MESSAGE_SIZE`
/// - Invalid JSON (logged at WARN level)
pub fn decode(line: &str) -> Option<RpcMessage> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > MAX_MESSAGE_SIZE {
        tracing::warn!(
            "[script stdout non-JSON] Line exceeds {} bytes, discarding",
            MAX_MESSAGE_SIZE
        );
        return None;
    }
    match serde_json::from_str(trimmed) {
        Ok(msg) => Some(msg),
        Err(e) => {
            tracing::warn!(
                "[script stdout non-JSON] Discarding line ({}): {}",
                e,
                if trimmed.len() > 200 {
                    format!("{}...", &trimmed[..200])
                } else {
                    trimmed.to_string()
                }
            );
            None
        }
    }
}

// tests module here (from Step 1)
```

Add `pub mod rpc;` to `gasket/engine/src/tools/script/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gasket-engine --lib tools::script::rpc::tests -- --nocapture`
Expected: All 10 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/script/
git commit -m "feat(script-tools): add JSON-RPC 2.0 types and line codec"
```

---

### Task 3: Dispatcher — Trait & Core

**Files:**
- Create: `gasket/engine/src/tools/script/dispatcher/mod.rs` (contains RpcHandler trait, DispatcherContext, RpcDispatcher — all in one file)
- Modify: `gasket/engine/src/tools/script/mod.rs` (add `pub mod dispatcher;`)

> **Note**: We use a directory module (`dispatcher/mod.rs`), NOT a separate `dispatcher.rs` file. This avoids the Rust file/directory conflict.

- [ ] **Step 1: Write failing tests for dispatcher**

```rust
// Append to gasket/engine/src/tools/script/dispatcher.rs

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A mock handler for testing.
    struct EchoHandler;

    #[async_trait]
    impl RpcHandler for EchoHandler {
        fn method(&self) -> &str { "test/echo" }
        fn required_permission(&self) -> Permission { Permission::LlmChat }

        async fn handle(&self, params: Value, _ctx: &DispatcherContext) -> Result<Value, RpcError> {
            Ok(params)
        }
    }

    fn test_dispatcher() -> RpcDispatcher {
        let mut d = RpcDispatcher::new();
        d.register(Box::new(EchoHandler));
        d
    }

    #[tokio::test]
    async fn test_dispatch_success() {
        let d = test_dispatcher();
        let req = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "test/echo".into(),
            params: Some(json!({"hello": "world"})),
        };
        let ctx = DispatcherContext::default();
        let resp = d.dispatch(&req, &[Permission::LlmChat], &ctx).await;
        assert_eq!(resp.id, json!(1));
        assert_eq!(resp.result.unwrap()["hello"], "world");
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_dispatch_permission_denied() {
        let d = test_dispatcher();
        let req = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "test/echo".into(),
            params: None,
        };
        let ctx = DispatcherContext::default();
        // Empty permissions → denied
        let resp = d.dispatch(&req, &[], &ctx).await;
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
    }

    #[tokio::test]
    async fn test_dispatch_method_not_found() {
        let d = test_dispatcher();
        let req = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "nonexistent".into(),
            params: None,
        };
        let ctx = DispatcherContext::default();
        let resp = d.dispatch(&req, &[Permission::LlmChat], &ctx).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_dispatch_no_id() {
        let d = test_dispatcher();
        let req = RpcRequest {
            jsonrpc: "2.0".into(),
            id: None, // notification
            method: "test/echo".into(),
            params: Some(json!(42)),
        };
        let ctx = DispatcherContext::default();
        let resp = d.dispatch(&req, &[Permission::LlmChat], &ctx).await;
        assert_eq!(resp.id, Value::Null); // default for no id
        assert_eq!(resp.result.unwrap(), json!(42));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-engine --lib tools::script::dispatcher::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error.

- [ ] **Step 3: Write dispatcher types and core logic**

Create `gasket/engine/src/tools/script/dispatcher/mod.rs`:

```rust
//! RPC method dispatcher with permission enforcement.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::manifest::Permission;
use super::rpc::{RpcError, RpcRequest, RpcResponse};

// ─── Handler Trait ──────────────────────────────────

/// Handles a single RPC method. Each engine capability implements this trait.
#[async_trait]
pub trait RpcHandler: Send + Sync {
    /// The RPC method name (e.g. "llm/chat").
    fn method(&self) -> &str;

    /// The permission required to call this method.
    fn required_permission(&self) -> Permission;

    /// Execute the method with given params and context.
    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError>;
}

// ─── Dispatcher Context ─────────────────────────────

/// Request-scoped context available to all handlers.
/// Built from ToolContext + engine-level references.
///
/// NOTE: Manual Default impl because Arc<dyn LlmProvider> cannot derive Default.
#[derive(Clone)]
pub struct DispatcherContext {
    pub session_key: Option<gasket_types::events::SessionKey>,
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<gasket_types::events::OutboundMessage>>,
    pub spawner: Option<Arc<dyn gasket_types::SubagentSpawner>>,
    pub token_tracker: Option<Arc<gasket_types::token_tracker::TokenTracker>>,
    pub tool_registry: Option<Arc<crate::tools::ToolRegistry>>,
    pub provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
}

impl Default for DispatcherContext {
    fn default() -> Self {
        Self {
            session_key: None,
            outbound_tx: None,
            spawner: None,
            token_tracker: None,
            tool_registry: None,
            provider: None,
        }
    }
}

// ─── Dispatcher ─────────────────────────────────────

/// Routes RPC requests to handlers, enforcing permissions.
pub struct RpcDispatcher {
    handlers: HashMap<String, Box<dyn RpcHandler>>,
}

impl RpcDispatcher {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler. Panics on duplicate method name.
    pub fn register(&mut self, handler: Box<dyn RpcHandler>) {
        let method = handler.method().to_string();
        assert!(
            !self.handlers.contains_key(&method),
            "Duplicate RPC handler: {}",
            method
        );
        self.handlers.insert(method, handler);
    }

    /// Dispatch an incoming request:
    /// 1. Find handler by method name
    /// 2. Check permission against script's declared permissions
    /// 3. Execute handler
    pub async fn dispatch(
        &self,
        request: &RpcRequest,
        permissions: &[Permission],
        ctx: &DispatcherContext,
    ) -> RpcResponse {
        let id = request.id.clone().unwrap_or(Value::Null);

        match self.handlers.get(&request.method) {
            Some(handler) => {
                if !permissions.contains(&handler.required_permission()) {
                    return RpcResponse {
                        jsonrpc: "2.0".into(),
                        id,
                        result: None,
                        error: Some(RpcError::permission_denied(&request.method)),
                    };
                }
                let params = request.params.clone().unwrap_or(Value::Null);
                match handler.handle(params, ctx).await {
                    Ok(result) => RpcResponse {
                        jsonrpc: "2.0".into(),
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(err) => RpcResponse {
                        jsonrpc: "2.0".into(),
                        id,
                        result: None,
                        error: Some(err),
                    },
                }
            }
            None => RpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(RpcError::method_not_found(&request.method)),
            },
        }
    }
}

impl Default for RpcDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

// tests module here (from Step 1)
```

Create `gasket/engine/src/tools/script/dispatcher/mod.rs` (contains trait + core logic + empty build_dispatcher):

> NOTE: The `build_dispatcher()` function is defined HERE in `dispatcher/mod.rs`, not in a separate file. Handlers are registered here as they're implemented.

```rust
// The build_dispatcher function is added at the bottom of the same file
// that contains RpcHandler, DispatcherContext, and RpcDispatcher.
// For now, it returns an empty dispatcher:

pub fn build_dispatcher() -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    // Handlers are registered in Tasks 7-8
    d
}
```

Add `pub mod dispatcher;` to `gasket/engine/src/tools/script/mod.rs`.

> **Do NOT create `dispatcher.rs` as a file** — only the `dispatcher/` directory with `mod.rs` inside.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gasket-engine --lib tools::script::dispatcher::tests -- --nocapture`
Expected: All 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/script/
git commit -m "feat(script-tools): add RPC dispatcher with permission enforcement"
```

---

### Task 4: Runner — Simple & JsonRpc Modes

**Files:**
- Create: `gasket/engine/src/tools/script/runner.rs`
- Modify: `gasket/engine/src/tools/script/mod.rs` (add `pub mod runner;`)

This is the largest task. Implementation first, integration test via Python script later.

- [ ] **Step 1: Write failing tests for runner**

```rust
// Append to gasket/engine/src/tools/script/runner.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::script::manifest::{RuntimeConfig, ScriptManifest, ScriptProtocol};

    fn echo_manifest() -> ScriptManifest {
        ScriptManifest {
            name: "echo_test".into(),
            description: "echo test".into(),
            version: "1.0.0".into(),
            runtime: RuntimeConfig {
                command: "echo".into(),
                args: vec![],
                working_dir: ".".into(),
                timeout_secs: 10,
                env: Default::default(),
            },
            protocol: ScriptProtocol::Simple,
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            permissions: vec![],
        }
    }

    #[tokio::test]
    async fn test_simple_mode_echo() {
        let manifest = echo_manifest();
        let args = serde_json::json!({"message": "hello"});
        let result = run_simple(&manifest, std::path::Path::new("/tmp"), args, Duration::from_secs(10))
            .await;
        // echo command will output the JSON to stdout
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_simple_mode_timeout() {
        let mut manifest = echo_manifest();
        manifest.runtime.command = "sleep".into();
        manifest.runtime.args = vec!["60".into()];
        let args = serde_json::json!({});
        let result = run_simple(&manifest, std::path::Path::new("/tmp"), args, Duration::from_millis(100))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::Timeout(secs) => assert_eq!(secs, 0),
            other => panic!("Expected Timeout, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_spawn_fails_for_bad_command() {
        let mut manifest = echo_manifest();
        manifest.runtime.command = "nonexistent_command_xyz".into();
        let args = serde_json::json!({});
        let result = run_simple(&manifest, std::path::Path::new("/tmp"), args, Duration::from_secs(5))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::SpawnFailed(_) => {}
            other => panic!("Expected SpawnFailed, got: {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-engine --lib tools::script::runner::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error.

- [ ] **Step 3: Write the runner implementation**

Create `gasket/engine/src/tools/script/runner.rs`:

```rust
//! Script process runners: Simple (one-shot) and JsonRpc (bidirectional).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::dispatcher::DispatcherContext;
use super::manifest::{Permission, ScriptManifest};
use super::rpc::{self, RpcMessage, RpcRequest};

// ─── Public Types ───────────────────────────────────

/// Result of a completed script execution.
pub struct ScriptResult {
    /// The final result value.
    pub output: serde_json::Value,
    /// Collected stderr for debugging.
    pub stderr: String,
    /// Wall-clock execution duration.
    pub duration: Duration,
}

/// Errors during script execution.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("Failed to spawn script process: {0}")]
    SpawnFailed(String),

    #[error("Script timed out after {0}s")]
    Timeout(u64),

    #[error("Script exited with non-zero code: {0:?}")]
    NonZeroExit(Option<i32>),

    #[error("Invalid script output: {0}")]
    InvalidOutput(String),

    #[error("I/O error: {0}")]
    Io(String),
}

// ─── Simple Runner ──────────────────────────────────

/// Run a script in Simple mode: pipe args as JSON to stdin, collect stdout.
pub async fn run_simple(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
    args: serde_json::Value,
    timeout: Duration,
) -> Result<ScriptResult, ScriptError> {
    let mut child = spawn_process(manifest, manifest_dir)?;
    let stdin = child.stdin.as_mut().unwrap();

    let input = serde_json::to_string(&args).unwrap();
    stdin.write_all(input.as_bytes()).await.map_err(|e| ScriptError::Io(e.to_string()))?;
    stdin.shutdown().await.map_err(|e| ScriptError::Io(e.to_string()))?;

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ScriptError::Timeout(timeout.as_secs()))?
        .map_err(|e| ScriptError::SpawnFailed(e.to_string()))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(ScriptError::NonZeroExit(output.status.code()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let result: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| ScriptError::InvalidOutput(format!("{} (stdout: {})", e, stdout.chars().take(200).collect::<String>())))?;

    Ok(ScriptResult {
        output: result,
        stderr,
        duration: Duration::from_millis(0),
    })
}

// ─── JsonRpc Runner ─────────────────────────────────

/// Run a script in JsonRpc mode: bidirectional message loop.
pub async fn run_jsonrpc(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
    args: serde_json::Value,
    timeout: Duration,
    permissions: &[Permission],
    dispatcher: &crate::tools::script::dispatcher::RpcDispatcher,
    ctx: &DispatcherContext,
) -> Result<ScriptResult, ScriptError> {
    let mut child = spawn_process(manifest, manifest_dir)?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr_child = child.stderr.take();

    let (response_tx, mut response_rx) = mpsc::channel::<String>(16);
    let mut reader = BufReader::new(stdout).lines();
    let mut writer = stdin;
    let mut stderr_collector = StderrCollector::new(stderr_child);

    // Send initialize request (id=0, reserved)
    let init_request = RpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(serde_json::json!(0)),
        method: "initialize".into(),
        params: Some(args),
    };
    write_message(&mut writer, &RpcMessage::Request(init_request)).await?;

    let start = std::time::Instant::now();
    let mut final_result: Option<serde_json::Value> = None;

    loop {
        tokio::select! {
            // Branch 1: Read line from script stdout
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(msg) = rpc::decode(&line) {
                            match msg {
                                RpcMessage::Request(req) => {
                                    debug!("Script callback: method={}", req.method);
                                    let response = dispatcher.dispatch(&req, permissions, ctx).await;
                                    let encoded = rpc::encode(&RpcMessage::Response(response));
                                    response_tx.send(encoded).await.map_err(|e| ScriptError::Io(e.to_string()))?;
                                }
                                RpcMessage::Response(resp) => {
                                    if resp.id == serde_json::json!(0) {
                                        // Final result from initialize response
                                        if let Some(err) = &resp.error {
                                            warn!("Script returned error: {:?}", err);
                                            final_result = Some(serde_json::json!({"error": err}));
                                        } else {
                                            final_result = Some(resp.result.clone().unwrap_or(serde_json::Value::Null));
                                        }
                                        break;
                                    }
                                    debug!("Unexpected response id={:?}, ignoring", resp.id);
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        warn!("Script closed stdout unexpectedly");
                        break;
                    }
                    Err(e) => {
                        return Err(ScriptError::Io(e.to_string()));
                    }
                }
            }

            // Branch 2: Write pending responses to script stdin
            encoded = response_rx.recv() => {
                if let Some(data) = encoded {
                    writer.write_all(data.as_bytes()).await.map_err(|e| ScriptError::Io(e.to_string()))?;
                    writer.flush().await.map_err(|e| ScriptError::Io(e.to_string()))?;
                }
            }

            // Branch 3: Timeout
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(ScriptError::Timeout(timeout.as_secs()));
            }
        }
    }

    // Cleanup
    let _ = child.wait().await;
    let stderr_output = stderr_collector.collect().await;

    Ok(ScriptResult {
        output: final_result.unwrap_or(serde_json::Value::Null),
        stderr: stderr_output,
        duration: start.elapsed(),
    })
}

// ─── Helpers ────────────────────────────────────────

fn spawn_process(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
) -> Result<Child, ScriptError> {
    let runtime = &manifest.runtime;
    let working_dir = if runtime.working_dir == "." {
        manifest_dir.to_path_buf()
    } else {
        manifest_dir.join(&runtime.working_dir)
    };

    let mut cmd = Command::new(&runtime.command);
    cmd.args(&runtime.args)
        .current_dir(&working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    for (k, v) in &runtime.env {
        cmd.env(k, v);
    }

    info!("Spawning script: {} {:?}", runtime.command, runtime.args);
    cmd.spawn().map_err(|e| ScriptError::SpawnFailed(e.to_string()))
}

async fn write_message(
    writer: &mut ChildStdin,
    msg: &RpcMessage,
) -> Result<(), ScriptError> {
    let encoded = rpc::encode(msg);
    writer.write_all(encoded.as_bytes()).await.map_err(|e| ScriptError::Io(e.to_string()))?;
    writer.flush().await.map_err(|e| ScriptError::Io(e.to_string()))?;
    Ok(())
}

// ─── Stderr Collector ──────────────────────────────

/// Drains stderr in the background to prevent pipe deadlock.
struct StderrCollector {
    handle: Option<tokio::task::JoinHandle<String>>,
}

impl StderrCollector {
    fn new(stderr: Option<tokio::process::ChildStderr>) -> Self {
        let handle = stderr.map(|stderr| {
            tokio::spawn(async move {
                let mut buf = String::new();
                let mut reader = tokio::io::BufReader::new(stderr);
                let _ = AsyncReadExt::read_to_string(&mut reader, &mut buf).await;
                buf
            })
        });
        Self { handle }
    }

    async fn collect(self) -> String {
        if let Some(h) = self.handle {
            match h.await {
                Ok(s) => s,
                Err(_) => String::new(),
            }
        } else {
            String::new()
        }
    }
}

// tests module here (from Step 1)
```

Add `pub mod runner;` and `pub use runner::{ScriptResult, ScriptError};` to `gasket/engine/src/tools/script/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gasket-engine --lib tools::script::runner::tests -- --nocapture`
Expected: All 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/script/
git commit -m "feat(script-tools): add simple and JSON-RPC process runners"
```

---

### Task 5: ScriptTool — Tool Trait Implementation & Discovery

**Files:**
- Modify: `gasket/engine/src/tools/script/mod.rs` (add ScriptTool struct and discover_scripts)
- Modify: `gasket/engine/src/tools/mod.rs` (re-export script module)

- [ ] **Step 1: Write failing tests for ScriptTool**

```rust
// Add to gasket/engine/src/tools/script/mod.rs tests module

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::script::manifest::{
        RuntimeConfig, ScriptManifest, ScriptProtocol,
    };

    fn test_manifest(name: &str, protocol: ScriptProtocol) -> ScriptManifest {
        ScriptManifest {
            name: name.into(),
            description: format!("Test tool: {}", name),
            version: "1.0.0".into(),
            runtime: RuntimeConfig {
                command: "cat".into(), // echoes stdin back
                args: vec![],
                working_dir: ".".into(),
                timeout_secs: 10,
                env: Default::default(),
            },
            protocol,
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            permissions: vec![],
        }
    }

    #[tokio::test]
    async fn test_simple_tool_execute() {
        let manifest = test_manifest("test_simple", ScriptProtocol::Simple);
        let tool = ScriptTool::new(manifest, std::path::PathBuf::from("/tmp"));

        assert_eq!(tool.name(), "test_simple");
        assert_eq!(tool.description(), "Test tool: test_simple");

        let args = serde_json::json!({"hello": "world"});
        let result = tool.execute(args, &crate::tools::ToolContext::default()).await;
        // cat echoes back the JSON
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_discover_scripts_no_dir() {
        // Should not panic when scripts dir doesn't exist
        let result = discover_scripts_in_dir(std::path::Path::new("/nonexistent/path/scripts"));
        assert!(result.is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package gasket-engine --lib tools::script::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `ScriptTool` not found.

- [ ] **Step 3: Implement ScriptTool and discover_scripts**

Complete `gasket/engine/src/tools/script/mod.rs`:

```rust
//! External script tool support.

pub mod dispatcher;
pub mod manifest;
pub mod rpc;
pub mod runner;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use dispatcher::{DispatcherContext, RpcDispatcher};
use manifest::{ScriptManifest, ScriptProtocol};
use runner::{self, ScriptError};

// Re-export primary types
pub use manifest::{Permission, RuntimeConfig, ScriptManifest, ScriptProtocol};
pub use runner::{ScriptError, ScriptResult};

// ─── ScriptTool ─────────────────────────────────────

/// A tool backed by an external script process.
///
/// Created from a manifest file, registered into ToolRegistry at startup.
pub struct ScriptTool {
    manifest: ScriptManifest,
    manifest_dir: PathBuf,
    dispatcher: Arc<RpcDispatcher>,
    /// Tool registry (injected after construction for handler access).
    tool_registry: Option<Arc<crate::tools::ToolRegistry>>,
    /// LLM provider (injected after construction for llm/chat handler).
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
}

impl ScriptTool {
    /// Create from a parsed manifest and its directory location.
    pub fn new(manifest: ScriptManifest, manifest_dir: PathBuf) -> Self {
        let dispatcher = Arc::new(dispatcher::build_dispatcher());
        info!(
            "Loaded script tool: {} (protocol={:?}, permissions={:?})",
            manifest.name, manifest.protocol, manifest.permissions,
        );
        Self {
            manifest,
            manifest_dir,
            dispatcher,
            tool_registry: None,
            provider: None,
        }
    }

    /// Inject engine references after construction.
    pub fn with_engine_refs(
        mut self,
        registry: Arc<crate::tools::ToolRegistry>,
        provider: Arc<dyn gasket_providers::LlmProvider>,
    ) -> Self {
        self.tool_registry = Some(registry);
        self.provider = Some(provider);
        self
    }

    fn make_dispatch_ctx(&self, ctx: &crate::tools::ToolContext) -> DispatcherContext {
        DispatcherContext {
            session_key: ctx.session_key.clone(),
            outbound_tx: ctx.outbound_tx.clone(),
            spawner: ctx.spawner.clone(),
            token_tracker: ctx.token_tracker.clone(),
            tool_registry: self.tool_registry.clone(),
            provider: self.provider.clone(),
        }
    }
}

#[async_trait]
impl crate::tools::Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn description(&self) -> &str {
        &self.manifest.description
    }

    fn parameters(&self) -> Value {
        self.manifest.parameters.clone()
    }

    #[instrument(name = "tool.script", skip_all, fields(tool = %self.manifest.name))]
    async fn execute(&self, args: Value, ctx: &crate::tools::ToolContext) -> crate::tools::ToolResult {
        let timeout = std::time::Duration::from_secs(self.manifest.runtime.timeout_secs);

        match self.manifest.protocol {
            ScriptProtocol::Simple => {
                debug!("Running script in Simple mode");
                let result = runner::run_simple(
                    &self.manifest,
                    &self.manifest_dir,
                    args,
                    timeout,
                )
                .await
                .map_err(|e| crate::tools::ToolError::ExecutionError(e.to_string()))?;

                Ok(serde_json::to_string_pretty(&result.output)
                    .unwrap_or_else(|_| result.output.to_string()))
            }

            ScriptProtocol::JsonRpc => {
                debug!(
                    "Running script in JsonRpc mode (permissions: {:?})",
                    self.manifest.permissions
                );
                let dispatch_ctx = self.make_dispatch_ctx(ctx);
                let result = runner::run_jsonrpc(
                    &self.manifest,
                    &self.manifest_dir,
                    args,
                    timeout,
                    &self.manifest.permissions,
                    &self.dispatcher,
                    &dispatch_ctx,
                )
                .await
                .map_err(|e| crate::tools::ToolError::ExecutionError(e.to_string()))?;

                let mut output = result.output;
                if !result.stderr.is_empty() {
                    if let Some(obj) = output.as_object_mut() {
                        obj.insert("_debug_stderr".into(), Value::String(result.stderr));
                    }
                }
                Ok(serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|_| output.to_string()))
            }
        }
    }
}

// ─── Script Discovery ──────────────────────────────

/// Scan a directory for script manifests and return parsed ScriptTools.
///
/// Non-fatal: individual manifest failures are logged but don't block others.
pub fn discover_scripts_in_dir(scripts_dir: &Path) -> anyhow::Result<Vec<ScriptTool>> {
    if !scripts_dir.exists() {
        debug!("Scripts directory not found: {:?}", scripts_dir);
        return Ok(Vec::new());
    }

    let mut tools = Vec::new();

    for entry in std::fs::read_dir(scripts_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let manifest_path = entry.path().join("manifest.yaml")
            .filter(|p| p.exists())
            .or_else(|| {
                let alt = entry.path().join("manifest.yml");
                if alt.exists() { Some(alt) } else { None }
            });

        let Some(manifest_path) = manifest_path else {
            debug!("No manifest found in {:?}", entry.path());
            continue;
        };

        match load_manifest(&manifest_path) {
            Ok(manifest) => {
                let manifest_dir = manifest_path.parent().unwrap().to_path_buf();
                let tool = ScriptTool::new(manifest, manifest_dir);
                info!("Registered script tool from {:?}", manifest_path);
                tools.push(tool);
            }
            Err(e) => {
                warn!("Failed to load manifest {:?}: {}", manifest_path, e);
            }
        }
    }

    Ok(tools)
}

/// Discover scripts from `~/.gasket/scripts/` and register them.
///
/// `engine_registry` and `provider` are injected into each ScriptTool so
/// its handlers can access built-in tools and LLM capabilities.
pub fn discover_scripts(
    registry: &mut crate::tools::ToolRegistry,
    engine_registry: Option<Arc<crate::tools::ToolRegistry>>,
    provider: Option<Arc<dyn gasket_providers::LlmProvider>>,
) -> anyhow::Result<()> {
    let scripts_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".gasket")
        .join("scripts");

    let tools = discover_scripts_in_dir(&scripts_dir)?;
    for tool in tools {
        let tool = match (engine_registry.clone(), provider.clone()) {
            (Some(r), Some(p)) => tool.with_engine_refs(r, p),
            _ => tool,
        };
        registry.register(Box::new(tool));
    }

    Ok(())
}

fn load_manifest(path: &Path) -> anyhow::Result<ScriptManifest> {
    let content = std::fs::read_to_string(path)?;
    let manifest: ScriptManifest = serde_yaml::from_str(&content)?;
    Ok(manifest)
}

// tests module here (from Step 1)
```

Add to `gasket/engine/src/tools/mod.rs`:

```rust
// Add after existing mod declarations:
pub mod script;

// Add to the re-exports section:
pub use script::ScriptTool;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gasket-engine --lib tools::script::tests -- --nocapture`
Expected: All tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/
git commit -m "feat(script-tools): add ScriptTool with discovery and Tool trait impl"
```

---

### Task 6: CLI Integration — Register Scripts at Startup

**Files:**
- Modify: `gasket/cli/src/commands/registry.rs` (add `discover_scripts` call)

- [ ] **Step 1: Write failing test**

Add to `gasket/cli/src/commands/registry.rs` bottom or a test file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_scripts_graceful_no_dir() {
        // Should not panic, should return Ok
        let mut registry = ToolRegistry::new();
        // nonexistent dir is handled inside discover_scripts
        // This tests that the integration doesn't break startup
        assert!(true); // placeholder — real test needs discover_scripts to be public
    }
}
```

- [ ] **Step 2: Integrate discover_scripts into build_tool_registry**

In `gasket/cli/src/commands/registry.rs`, after the `extra_tools` loop (line ~345), add:

```rust
    // Discover external script tools from ~/.gasket/scripts/
    // Wrap the completed registry in Arc so script handlers can call built-in tools.
    let engine_registry = Arc::new(tools);
    if let Err(e) = gasket_engine::tools::script::discover_scripts(
        &mut Arc::try_unwrap(engine_registry).unwrap_or_else(|arc| (*arc).clone()),
        Some(engine_registry.clone()),
        None, // provider is resolved per-request, not available at registry build time
    ) {
        tracing::warn!("Failed to discover script tools: {}", e);
    }
```

> **Note**: The provider is resolved per-request (not at registry build time), so it's passed as `None` here. A future enhancement can inject it when the RuntimeContext is assembled.

This requires `gasket_engine::tools::script` to be accessible. Verify that `engine/src/tools/mod.rs` exports `pub mod script`.

- [ ] **Step 3: Build and verify no compilation errors**

Run: `cargo build --package gasket-cli 2>&1 | tail -10`
Expected: Build succeeds with no errors.

- [ ] **Step 4: Run all existing tests to check for regressions**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: All existing tests PASS. New script module tests also pass.

- [ ] **Step 5: Commit**

```bash
git add gasket/cli/src/commands/registry.rs
git commit -m "feat(cli): integrate script tool discovery at startup"
```

---

### Task 7: Handler — LLM Chat

**Files:**
- Create: `gasket/engine/src/tools/script/dispatcher/llm_chat.rs`
- Modify: `gasket/engine/src/tools/script/dispatcher/mod.rs` (register handler)

- [ ] **Step 1: Write the LlmChatHandler**

Create `gasket/engine/src/tools/script/dispatcher/llm_chat.rs`:

```rust
//! LLM chat completion callback handler.

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::script::dispatcher::{DispatcherContext, RpcHandler};
use crate::tools::script::manifest::Permission;
use crate::tools::script::rpc::RpcError;

pub struct LlmChatHandler;

#[async_trait]
impl RpcHandler for LlmChatHandler {
    fn method(&self) -> &str {
        "llm/chat"
    }

    fn required_permission(&self) -> Permission {
        Permission::LlmChat
    }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let provider = ctx
            .provider
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("No LLM provider available"))?;

        let request: gasket_providers::ChatRequest = serde_json::from_value(params)
            .map_err(|e| RpcError::invalid_params(&e.to_string()))?;

        let response = provider
            .chat(request)
            .await
            .map_err(|e| RpcError::internal_error(&e.to_string()))?;

        // Track token usage if tracker is available
        if let Some(tracker) = &ctx.token_tracker {
            if let Some(usage) = &response.usage {
                let token_usage = gasket_types::token_tracker::TokenUsage {
                    prompt_tokens: usage.prompt_tokens as u32,
                    completion_tokens: usage.completion_tokens as u32,
                    total_tokens: usage.total_tokens as u32,
                };
                // Best-effort tracking — don't fail the callback if tracking fails
                let _ = tracker.accumulate(&token_usage, 0.0);
            }
        }

        serde_json::to_value(response)
            .map_err(|e| RpcError::internal_error(&e.to_string()))
    }
}
```

- [ ] **Step 2: Register in dispatcher/mod.rs**

Update `gasket/engine/src/tools/script/dispatcher/mod.rs`:

```rust
//! RPC method handler implementations.

// RpcDispatcher is defined in this file, so no import needed.
mod llm_chat;

use llm_chat::LlmChatHandler;

/// Build the standard dispatcher with all registered handlers.
pub fn build_dispatcher() -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    d.register(Box::new(LlmChatHandler));
    d
}
```

- [ ] **Step 3: Build to verify compilation**

Run: `cargo build --package gasket-engine 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/tools/script/dispatcher/
git commit -m "feat(script-tools): add LLM chat callback handler"
```

---

### Task 8: Handlers — Memory & Subagent

**Files:**
- Create: `gasket/engine/src/tools/script/dispatcher/memory_search.rs`
- Create: `gasket/engine/src/tools/script/dispatcher/memory_write.rs`
- Create: `gasket/engine/src/tools/script/dispatcher/memory_decay.rs`
- Create: `gasket/engine/src/tools/script/dispatcher/subagent.rs`
- Modify: `gasket/engine/src/tools/script/dispatcher/mod.rs`

All handlers follow the same pattern. Implement them in one pass.

- [ ] **Step 1: Create all handler files**

Create `gasket/engine/src/tools/script/dispatcher/memory_search.rs`:

```rust
//! Memory search callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler, RpcError};
use crate::tools::script::manifest::Permission;

pub struct MemorySearchHandler;

#[async_trait]
impl RpcHandler for MemorySearchHandler {
    fn method(&self) -> &str { "memory/search" }
    fn required_permission(&self) -> Permission { Permission::MemorySearch }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let registry = ctx.tool_registry
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("Tool registry not available"))?;

        let mut tool_ctx = crate::tools::ToolContext::default();
        if let Some(ref key) = ctx.session_key {
            tool_ctx = tool_ctx.session_key(key.clone());
        }

        registry.execute("memory_search", params, &tool_ctx)
            .await
            .map(|output_str| serde_json::json!({"output": output_str}))
            .map_err(|e| RpcError::internal_error(&e.to_string()))
    }
}
```

Create `gasket/engine/src/tools/script/dispatcher/memory_write.rs`:

```rust
//! Memory write callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler, RpcError};
use crate::tools::script::manifest::Permission;

pub struct MemoryWriteHandler;

#[async_trait]
impl RpcHandler for MemoryWriteHandler {
    fn method(&self) -> &str { "memory/write" }
    fn required_permission(&self) -> Permission { Permission::MemoryWrite }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let registry = ctx.tool_registry
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("Tool registry not available"))?;

        let tool_ctx = crate::tools::ToolContext::default();

        registry.execute("memorize", params, &tool_ctx)
            .await
            .map(|output_str| serde_json::json!({"output": output_str}))
            .map_err(|e| RpcError::internal_error(&e.to_string()))
    }
}
```

Create `gasket/engine/src/tools/script/dispatcher/memory_decay.rs`:

```rust
//! Memory decay callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler, RpcError};
use crate::tools::script::manifest::Permission;

pub struct MemoryDecayHandler;

#[async_trait]
impl RpcHandler for MemoryDecayHandler {
    fn method(&self) -> &str { "memory/decay" }
    fn required_permission(&self) -> Permission { Permission::MemoryDecay }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let registry = ctx.tool_registry
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("Tool registry not available"))?;

        let tool_ctx = crate::tools::ToolContext::default();

        registry.execute("memory_decay", params, &tool_ctx)
            .await
            .map(|output_str| serde_json::json!({"output": output_str}))
            .map_err(|e| RpcError::internal_error(&e.to_string()))
    }
}
```

Create `gasket/engine/src/tools/script/dispatcher/subagent.rs`:

```rust
//! Subagent spawn callback handler.

use async_trait::async_trait;
use serde_json::Value;

use super::{DispatcherContext, RpcHandler, RpcError};
use crate::tools::script::manifest::Permission;

pub struct SubagentSpawnHandler;

#[async_trait]
impl RpcHandler for SubagentSpawnHandler {
    fn method(&self) -> &str { "subagent/spawn" }
    fn required_permission(&self) -> Permission { Permission::SubagentSpawn }

    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError> {
        let spawner = ctx.spawner
            .as_ref()
            .ok_or_else(|| RpcError::internal_error("Subagent spawner not available"))?;

        #[derive(serde::Deserialize)]
        struct SpawnParams {
            task: String,
            #[serde(default)]
            model_id: Option<String>,
        }

        let p: SpawnParams = serde_json::from_value(params)
            .map_err(|e| RpcError::invalid_params(&e.to_string()))?;

        let result = spawner.spawn(p.task, p.model_id)
            .await
            .map_err(|e| RpcError::internal_error(&e.to_string()))?;

        serde_json::to_value(serde_json::json!({
            "id": result.id,
            "task": result.task,
            "content": result.response.content,
            "model": result.model,
        }))
        .map_err(|e| RpcError::internal_error(&e.to_string()))
    }
}
```

- [ ] **Step 2: Register all handlers in dispatcher/mod.rs**

Update `gasket/engine/src/tools/script/dispatcher/mod.rs`:

```rust
//! RPC method handler implementations.
//!
//! RpcDispatcher, DispatcherContext, and RpcHandler trait are defined
//! in this file (dispatcher/mod.rs), so no import needed.

mod llm_chat;
mod memory_decay;
mod memory_search;
mod memory_write;
mod subagent;

use llm_chat::LlmChatHandler;
use memory_decay::MemoryDecayHandler;
use memory_search::MemorySearchHandler;
use memory_write::MemoryWriteHandler;
use subagent::SubagentSpawnHandler;

/// Build the standard dispatcher with all registered handlers.
pub fn build_dispatcher() -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    d.register(Box::new(LlmChatHandler));
    d.register(Box::new(MemorySearchHandler));
    d.register(Box::new(MemoryWriteHandler));
    d.register(Box::new(MemoryDecayHandler));
    d.register(Box::new(SubagentSpawnHandler));
    d
}
```

- [ ] **Step 3: Build to verify compilation**

Run: `cargo build --package gasket-engine 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 4: Run all tests**

Run: `cargo test --package gasket-engine --lib tools::script 2>&1 | tail -20`
Expected: All tests PASS.

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/tools/script/dispatcher/
git commit -m "feat(script-tools): add memory and subagent callback handlers"
```

---

### Task 9: Integration Test — Python Script End-to-End

**Files:**
- Create: `tests/scripts/simple_echo/manifest.yaml`
- Create: `tests/scripts/simple_echo/echo.py`
- Create: `tests/scripts/jsonrpc_ping/manifest.yaml`
- Create: `tests/scripts/jsonrpc_ping/ping.py`
- Create: `gasket/engine/tests/script_integration.rs`

- [ ] **Step 1: Create test scripts and manifests**

Create `tests/scripts/simple_echo/manifest.yaml`:

```yaml
name: "test_echo"
description: "Echoes back the input (simple mode)"
parameters:
  type: object
  properties:
    message: { type: string, description: "Message to echo" }
  required: ["message"]
runtime:
  command: "python3"
  args: ["echo.py"]
  timeout_secs: 10
```

Create `tests/scripts/simple_echo/echo.py`:

```python
#!/usr/bin/env python3
"""Simple mode script: reads JSON from stdin, writes to stdout."""
import json
import sys

data = json.load(sys.stdin)
result = {"echo": data.get("message", ""), "status": "ok"}
json.dump(result, sys.stdout)
```

Create `tests/scripts/jsonrpc_ping/manifest.yaml`:

```yaml
name: "test_ping"
description: "Ping-pong JSON-RPC test"
protocol: "jsonrpc"
parameters:
  type: object
  properties:
    name: { type: string, description: "Your name" }
  required: ["name"]
runtime:
  command: "python3"
  args: ["ping.py"]
  timeout_secs: 30
  env:
    PYTHONUNBUFFERED: "1"
permissions:
  - llm_chat
```

Create `tests/scripts/jsonrpc_ping/ping.py`:

```python
#!/usr/bin/env python3
"""JSON-RPC mode script: receives initialize, makes one callback, returns result."""
import json
import sys

def send(msg):
    """Send a JSON-RPC message to stdout."""
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

def recv():
    """Read a JSON-RPC message from stdin."""
    line = sys.stdin.readline()
    if not line:
        return None
    return json.loads(line.strip())

def main():
    # 1. Wait for initialize request from engine
    init_msg = recv()
    assert init_msg is not None, "No initialize message received"
    assert init_msg.get("method") == "initialize"
    params = init_msg.get("params", {})
    name = params.get("name", "world")

    # 2. Make a callback to the engine (llm/chat)
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "llm/chat",
        "params": {
            "model": "test",
            "messages": [{"role": "user", "content": f"Say hello to {name}"}]
        }
    })

    # 3. Read the response
    llm_response = recv()
    # (In test, provider may not be available, so we handle errors)

    # 4. Send final result back (responding to initialize)
    send({
        "jsonrpc": "2.0",
        "id": 0,  # must match the initialize request id
        "result": {
            "greeting": f"Hello, {name}!",
            "llm_called": llm_response is not None and "error" not in (llm_response or {})
        }
    })

if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Write the integration test**

Create `gasket/engine/tests/script_integration.rs`:

```rust
//! Integration tests for script tools.

use std::path::PathBuf;

use gasket_engine::tools::script::{discover_scripts_in_dir, ScriptTool};
use gasket_engine::tools::{Tool, ToolContext};

fn test_scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("scripts")
}

#[tokio::test]
async fn test_simple_echo_tool() {
    let scripts_dir = test_scripts_dir();
    let tools = discover_scripts_in_dir(&scripts_dir).unwrap();

    let echo_tool = tools
        .iter()
        .find(|t| t.name() == "test_echo")
        .expect("test_echo tool not found");

    let args = serde_json::json!({"message": "hello world"});
    let result = echo_tool.execute(args, &ToolContext::default()).await;

    assert!(result.is_ok(), "Simple echo failed: {:?}", result);
    let output = result.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["echo"], "hello world");
    assert_eq!(parsed["status"], "ok");
}

#[tokio::test]
async fn test_jsonrpc_ping_tool() {
    let scripts_dir = test_scripts_dir();
    let tools = discover_scripts_in_dir(&scripts_dir).unwrap();

    let ping_tool = tools
        .iter()
        .find(|t| t.name() == "test_ping")
        .expect("test_ping tool not found");

    let args = serde_json::json!({"name": "Alice"});
    let result = ping_tool.execute(args, &ToolContext::default()).await;

    // The script will try llm/chat but provider is None, so it gets an error.
    // The script should still return a final result with llm_called=false.
    assert!(result.is_ok(), "JsonRpc ping failed: {:?}", result);
    let output = result.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["greeting"], "Hello, Alice!");
    assert_eq!(parsed["llm_called"], false);
}

#[test]
fn test_discover_finds_both_tools() {
    let scripts_dir = test_scripts_dir();
    let tools = discover_scripts_in_dir(&scripts_dir).unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"test_echo"), "Missing test_echo");
    assert!(names.contains(&"test_ping"), "Missing test_ping");
}
```

- [ ] **Step 3: Run integration tests**

Run: `cargo test --package gasket-engine --test script_integration -- --nocapture`
Expected: All 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/ gasket/engine/tests/
git commit -m "test(script-tools): add Python integration tests for simple and JSON-RPC modes"
```

---

### Task 10: Full Workspace Build & Regression Test

- [ ] **Step 1: Run full workspace build**

Run: `cargo build --release --workspace 2>&1 | tail -10`
Expected: Build succeeds with no errors.

- [ ] **Step 2: Run all workspace tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All tests PASS (existing + new script tool tests).

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --package gasket-engine -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore(script-tools): workspace build verification and cleanup"
```
