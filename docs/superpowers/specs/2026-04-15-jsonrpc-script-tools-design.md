# JSON-RPC Script Tools Design

> **Date**: 2026-04-15
> **Status**: Draft
> **Scope**: Extend Gasket's tool system with external script support, using JSON-RPC 2.0 over JSON-Lines for bidirectional engine-script communication.

## 1. Problem Statement

Gasket's tool system is entirely implemented in Rust. Every tool (exec, memory_search, filesystem, etc.) is a compiled `Tool` trait implementation. This creates two problems:

1. **Extension friction** — Adding a new tool requires writing Rust, recompiling, and redeploying. Python/Node.js developers cannot extend Gasket without Rust expertise.
2. **No callback capability** — Scripts cannot access engine resources (LLM, memory, subagents) during execution. They receive input, run, and return output — but cannot ask the engine for help mid-computation.

## 2. Design Goals

1. **KISS** — JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout. No HTTP, no WebSocket, no gRPC. UNIX pipe philosophy.
2. **Backward compatible** — Existing one-shot scripts (stdin JSON in, stdout JSON out) continue to work via `protocol: "simple"`.
3. **Declarative permissions** — Scripts declare needed engine capabilities in their manifest. Undeclared capabilities are denied at the RPC layer.
4. **Short-lived processes** — Each tool invocation spawns a fresh child process. No daemon, no state leakage between calls.
5. **Crash-proof** — Invalid JSON on stdout is silently discarded. Script crashes, timeouts, and OOM are handled gracefully. The engine never panics due to a misbehaving script.
6. **Default-deny** — Scripts with no `permissions` field have zero engine callback capabilities. Every capability must be explicitly declared.

## 3. Architecture

```
~/.gasket/scripts/
  <tool_name>/manifest.yaml   ← Declarative config (protocol, permissions, params)
  <tool_name>/main.py         ← Script entry point

ToolRegistry
  ├── exec (ExecTool)              ← Built-in
  ├── memory_search (MemorySearchTool)  ← Built-in
  ├── web_analyzer (ScriptTool)    ← External script
  └── data_pipeline (ScriptTool)   ← External script

ScriptTool.execute()
  ├── protocol=simple  → run_simple()
  │     stdin(args) → process → stdout(result)
  └── protocol=jsonrpc → run_jsonrpc()
        Multiplexer (tokio::select!)
          ├── reader: stdout line → RpcMessage
          │     ├── Request  → RpcDispatcher → RpcHandler
          │     └── Response(id=init) → final result, exit loop
          ├── writer: response_tx → stdin
          └── timeout guard → kill process
```

## 4. Module Design

### 4.1 ScriptManifest

**File**: `engine/src/tools/script/manifest.rs`

Defines the YAML manifest structure, protocol selection, and permission model.

```yaml
# ~/.gasket/scripts/my_tool/manifest.yaml
name: "web_analyzer"
description: "Analyze web content and generate summaries"
version: "1.0.0"

runtime:
  command: "python3"
  args: ["main.py"]
  working_dir: "."
  timeout_secs: 120
  env:
    PYTHONUNBUFFERED: "1"

protocol: "jsonrpc"    # "simple" (default) | "jsonrpc"

parameters:
  type: object
  properties:
    url: { type: string, description: "URL to analyze" }
    depth: { type: integer, description: "Analysis depth" }
  required: ["url"]

permissions:
  - llm/chat
  - memory/search
```

Rust types:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptManifest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub protocol: ScriptProtocol,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScriptProtocol {
    #[default]
    Simple,
    JsonRpc,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_working_dir")]
    pub working_dir: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    LlmChat,
    MemorySearch,
    MemoryWrite,
    MemoryDecay,
    SubagentSpawn,
}
```

**Permission ↔ RPC method mapping**:

| Permission | RPC Method | Notes |
|-----------|-----------|-------|
| `LlmChat` | `llm/chat` | Uses `LlmProvider::chat()` |
| `MemorySearch` | `memory/search` | Reuses `MemorySearchTool` logic |
| `MemoryWrite` | `memory/write` | Reuses `MemorizeTool` logic |
| `MemoryDecay` | `memory/decay` | Reuses `MemoryDecayTool` logic |
| `SubagentSpawn` | `subagent/spawn` | Uses `SubagentSpawner::spawn()` |

> **Removed from MVP**: `LlmEmbed` (no `embed()` on `LlmProvider` trait), `ToolExec` (ambiguous — shell exec via script creates recursion concern). These can be added when the provider trait is extended or a clear use case emerges.

**Default behavior**: If `permissions` is omitted or empty, the script has **zero** callback capabilities (default-deny). Even in `jsonrpc` mode, the script can only receive the `initialize` request and return its final result.

### 4.2 RPC Types & Codec

**File**: `engine/src/tools/script/rpc.rs`

Standard JSON-RPC 2.0 message types with line-based codec.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcMessage {
    Request(RpcRequest),
    Response(RpcResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,           // "2.0"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,         // None = notification
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}
```

**Codec functions**:
- `encode(msg: &RpcMessage) -> String` — Serializes to JSON + appends `\n`
- `decode(line: &str) -> Option<RpcMessage>` — Deserializes a line. Returns `None` for non-JSON lines (logged at WARN level with prefix `[script stdout non-JSON]`, then discarded — NOT included in stderr).

**Message size limit**: Maximum 1 MiB per JSON-RPC message. Lines exceeding this are logged as warning and discarded (same as non-JSON). This prevents OOM from malicious or buggy scripts.

**Standard error codes**:
- `-32700`: Parse error
- `-32600`: Invalid Request
- `-32601`: Method not found
- `-32602`: Invalid params
- `-32603`: Internal error
- `-32000`: Permission denied (application-defined)

**`ToolError` → RPC error code mapping** (used by handlers that wrap built-in tools):

| ToolError variant | RPC code |
|------------------|----------|
| `InvalidArguments` | `-32602` |
| `PermissionDenied` | `-32000` |
| `NotFound` | `-32601` |
| `ExecutionError` | `-32603` |

### 4.3 RpcDispatcher

**File**: `engine/src/tools/script/dispatcher.rs`

Trait-based method dispatch with unified permission checking.

```rust
#[async_trait]
pub trait RpcHandler: Send + Sync {
    fn method(&self) -> &str;
    fn required_permission(&self) -> Permission;
    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError>;
}

/// Request-scoped context built from ToolContext + engine references.
/// Handlers use this to access engine capabilities.
pub struct DispatcherContext {
    pub session_key: Option<gasket_types::events::SessionKey>,
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<gasket_types::events::OutboundMessage>>,
    pub spawner: Option<std::sync::Arc<dyn gasket_types::SubagentSpawner>>,
    pub token_tracker: Option<std::sync::Arc<gasket_types::TokenTracker>>,
    pub tool_registry: Option<std::sync::Arc<super::ToolRegistry>>,
    /// LLM provider for chat callbacks (from engine's provider pool)
    pub provider: Option<std::sync::Arc<dyn gasket_providers::LlmProvider>>,
}

pub struct RpcDispatcher {
    handlers: HashMap<String, Box<dyn RpcHandler>>,
}
```

**Three-layer defense** in `dispatch()`:
1. **Method routing** — Unknown method → `-32601 Method not found`
2. **Permission check** — Method exists but undeclared → `-32000 Permission denied`
3. **Handler execution** — Invalid params → `-32602 Invalid params`

**Handler implementations** (one file per method):

| File | Handler | Reuses |
|------|---------|--------|
| `dispatcher/llm_chat.rs` | `LlmChatHandler` | `LlmProvider::chat()` |
| `dispatcher/memory_search.rs` | `MemorySearchHandler` | `MemorySearchTool` logic |
| `dispatcher/memory_write.rs` | `MemoryWriteHandler` | `MemorizeTool` logic |
| `dispatcher/memory_decay.rs` | `MemoryDecayHandler` | `MemoryDecayTool` logic |
| `dispatcher/subagent.rs` | `SubagentSpawnHandler` | `SubagentSpawner::spawn()` |

### 4.4 Multiplexer (Runner)

**File**: `engine/src/tools/script/runner.rs`

Two runner functions sharing process spawn logic:

#### `run_simple()`
Pipes args as JSON to stdin, collects stdout as JSON result. One-shot, no callback.

#### `run_jsonrpc()`
The core bidirectional loop:

```
1. Spawn child process with piped stdin/stdout/stderr
2. Start StderrCollector (background tokio::spawn to drain stderr)
3. Send "initialize" request (id=0, reserved) to script's stdin
4. Enter tokio::select! loop:
   Branch 1 (reader): Read line from stdout
     → RpcMessage::Request: dispatch to handler, write response to stdin
     → RpcMessage::Response with id=0: extract result, break loop
     → Invalid JSON: discard with warning log
   Branch 2 (writer): Write pending responses to stdin (bounded channel, capacity 16)
   Branch 3 (timeout): Kill process, return Timeout error
5. Wait for child exit, collect stderr
6. Return ScriptResult { output, stderr, duration }
```

**Reserved ID**: The engine uses `id: 0` (JSON number) for the `initialize` request. Scripts **MUST NOT** use `id: 0` for their own requests — they should use `id: 1, 2, 3, ...` or string IDs. This eliminates the sentinel value conflict.

**Timeout enforcement**: The `tokio::time::sleep(timeout)` branch in `select!` triggers regardless of I/O activity. Even if the script is stuck in a CPU loop with no stdout output, the process is killed when the timeout expires.

**Deadlock prevention**:
- Read and write are in separate `select!` branches — both directions proceed independently
- `StderrCollector` runs in a dedicated `tokio::spawn` — prevents stderr buffer full → script blocks → pipe deadlock
- Response channel is bounded (capacity 16) — if the script isn't reading stdin, the dispatch awaits backpressure rather than growing unbounded
- `kill_on_drop(true)` on `tokio::process::Command` ensures process cleanup on any exit path

### 4.5 ScriptTool (Tool trait implementation)

**File**: `engine/src/tools/script/mod.rs`

```rust
pub struct ScriptTool {
    manifest: ScriptManifest,
    manifest_dir: PathBuf,
    dispatcher: Arc<RpcDispatcher>,
}
```

Implements `Tool` trait:
- `name()` → `manifest.name`
- `description()` → `manifest.description`
- `parameters()` → `manifest.parameters`
- `execute()` → delegates to `run_simple()` or `run_jsonrpc()` based on `manifest.protocol`

**Script discovery** (`discover_scripts`):
1. Scan `~/.gasket/scripts/*/manifest.yaml` (or `.yml`)
2. Parse each manifest
3. Create `ScriptTool` and register into `ToolRegistry`
4. Failed manifests are logged but don't block other scripts

## 5. Protocol Flow

### 5.1 Simple Mode

```
Engine                              Script
  │                                   │
  │──── stdin: JSON args ────────────>│
  │                                   │ (processes)
  │<─── stdout: JSON result ──────────│
  │                                   │ (exits)

stderr is collected separately and attached to debug output.
```

### 5.2 JsonRpc Mode

```
Engine                              Script
  │                                   │
  │──── {"jsonrpc":"2.0","id":0,      │  ← reserved id=0
  │      "method":"initialize",       │
  │      "params":{...args...}}       │
  │──────────────────────────────────>│
  │                                   │
  │<─── {"jsonrpc":"2.0","id":1,      │  ← script uses id≥1
  │      "method":"llm/chat",         │
  │      "params":{...}}              │
  │                                   │
  │──── {"jsonrpc":"2.0","id":1,      │
  │      "result":{...llm response}}  │
  │──────────────────────────────────>│
  │                                   │
  │<─── {"jsonrpc":"2.0","id":0,      │  ← response to init
  │      "result":{...final answer}}  │
  │                                   │
  │  (script exits, process cleaned)  │

Convention: Engine uses id=0 for initialize. Scripts use id≥1 for callbacks.
Scripts MUST respond to id=0 with their final result to complete execution.
```

## 6. Security Model

### 6.1 Declarative Permissions (Default-Deny)

Scripts must declare all required engine capabilities in `manifest.yaml`. The dispatcher enforces this at the RPC layer:

- If `permissions` is omitted or empty → **zero** callback capabilities (default-deny)
- Undeclared method → `-32000 Permission denied` response
- Undeclared methods return the **same** error code as unknown methods (`-32601`) to prevent capability probing
- Only declared, registered methods return distinct error codes

### 6.2 Process Isolation

- Each invocation gets a fresh child process (short-lived, no state leakage between calls)
- Concurrent invocations of the same script are fully isolated (separate processes)
- `kill_on_drop(true)` ensures cleanup on any exit path (SIGTERM on Drop)
- Timeout enforcement via `tokio::select!` — kills process even if stuck in CPU loop
- Stderr is isolated from the JSON-RPC data stream

### 6.3 Resource Limits

- `timeout_secs` per invocation (default 120s)
- Maximum message size: 1 MiB per JSON-RPC message (prevents OOM)
- Response channel capacity: 16 messages (backpressure, not unbounded growth)
- Token budget enforcement via `TokenTracker` for LLM callbacks — handlers MUST call `ctx.token_tracker.add_usage()` after each LLM call
- Future: inherit sandbox resource limits from `RuntimeConfig` (memory/CPU)

### 6.4 working_dir Resolution

`working_dir` in the manifest is **always resolved relative to the script's manifest directory** (`~/.gasket/scripts/<tool_name>/`). A value of `"."` means the script's own directory. This ensures scripts can always find their local dependencies (Python modules, data files).

## 7. File Layout

```
engine/src/tools/script/
├── mod.rs                  # ScriptTool + discover_scripts (~150 lines)
├── manifest.rs             # Manifest structs + Permission enum (~90 lines)
├── rpc.rs                  # JSON-RPC types + codec (~130 lines)
├── dispatcher.rs           # RpcDispatcher trait + core (~130 lines)
├── dispatcher/
│   ├── mod.rs              # Handler re-exports
│   ├── llm_chat.rs         # ~40 lines
│   ├── memory_search.rs    # ~40 lines
│   ├── memory_write.rs     # ~40 lines
│   ├── memory_decay.rs     # ~30 lines
│   └── subagent.rs         # ~50 lines
└── runner.rs               # Simple + JsonRpc runners (~260 lines)

Total: ~960 lines
```

## 8. Testing Strategy

1. **Unit tests** — `manifest.rs`: manifest parsing, permission enum, default values, default-deny when permissions omitted
2. **Unit tests** — `rpc.rs`: encode/decode roundtrip, invalid JSON tolerance, message size limit enforcement, error code constants
3. **Unit tests** — `dispatcher.rs`: mock handler, permission denial, method not found, `ToolError` → RPC code mapping
4. **Integration test** — Spawn a Python script that echoes JSON-RPC messages, verify Rust receives and responds correctly
5. **Integration test** — Script calls `llm/chat` (mocked provider), verify response flows back
6. **Integration test** — Script exceeds timeout, verify process is killed and error returned
7. **Integration test** — Script writes garbage to stdout mid-session, verify engine doesn't crash
8. **Integration test** — Script exits with non-zero code before sending final response (id=0), verify engine returns appropriate error
9. **Integration test** — Script sends malformed JSON-RPC (missing `jsonrpc` field), verify graceful handling
10. **Integration test** — Concurrent invocations of the same script tool, verify process isolation

## 9. Future Considerations (Out of Scope)

- **Hot reload**: Watch `~/.gasket/scripts/` for manifest changes and re-register tools without restart
- **Long-lived daemon mode**: Optional `protocol: "jsonrpc-daemon"` for persistent processes
- **Streaming callbacks**: Allow scripts to receive SSE streaming from LLM responses
- **Custom RPC handlers**: User-defined handler registration for plugin extensibility
- **Sandbox integration**: Run script processes through `gasket-sandbox` for OS-level isolation
