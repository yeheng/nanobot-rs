# Architecture Overview

> Gasket-RS System Architecture Overview

---

## Crate Structure

```
gasket-rs/                    (Cargo workspace)
├── engine/                   Core orchestration crate — Agent engine, tools, Hook system
│   └── src/
│       ├── kernel/            Pure function execution core (executor, stream)
│       ├── session/           Session management (AgentSession, context, compactor, memory)
│       ├── subagents/         Subagent system (manager, tracker, runner)
│       ├── config/            Configuration loading (YAML → Struct)
│       ├── cron/              Scheduled task service
│       ├── heartbeat/         Heartbeat service
│       ├── hooks/             Pipeline Hook system
│       ├── skills/            Skills system
│       ├── tools/             Tool system (14 built-in tools)
│       └── vault/             Sensitive data isolation module
├── cli/                      CLI executable
│   └── src/
│       ├── main.rs            Command entry + Gateway launcher
│       ├── cli.rs             CLI interactive mode
│       ├── provider.rs        Provider factory
│       └── commands/          Subcommands (onboard, status, agent, gateway, channels, cron, vault, memory)
├── types/                    Shared type definitions (Tool trait, events, session_event, etc.)
├── providers/                LLM provider implementations
├── storage/                  SQLite storage + embedding + memory system
├── channels/                 Communication channel implementations
├── sandbox/                  Sandbox execution environment
└── tantivy/                  Tantivy search MCP server (standalone binary)
```

---

## System Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                        cli (Binary)                              │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌──────────┐ ┌─────────┐   │
│  │ onboard │ │ status  │ │  agent  │ │ gateway  │ │channels │   │
│  │  (init) │ │ (check) │ │  (CLI)  │ │ (daemon) │ │ status  │   │
│  └─────────┘ └─────────┘ └────┬────┘ └────┬─────┘ └─────────┘   │
└────────────────────────────────┼───────────┼─────────────────────┘
                                 │           │
─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┼ ─ ─ ─ ─ ─┼ ─ ─ ─ ─ ─ ─ ─ ─ ─
                                 │           │
┌────────────────────────────────┼───────────┼─────────────────────┐
│                        engine (Library)                          │
│                                │           │                     │
│  ┌─────────────────────────────▼───────────▼──────────────────┐  │
│  │                   AgentSession (Session Management)          │  │
│  │  ┌────────────┐  ┌──────────────┐  ┌──────────────────┐   │  │
│  │  │   Prompt   │  │    kernel    │  │    Session        │   │  │
│  │  │   Loader   │  │   execute    │  │   Management     │   │  │
│  │  └────────────┘  └──────────────┘  └──────────────────┘   │  │
│  │  ┌────────────────────┐  ┌────────────────────────────┐   │  │
│  │  │  Context Compactor │  │      Hook Registry         │   │  │
│  │  │  (sync compress)   │  │  (BeforeRequest/AfterResp) │   │  │
│  │  └────────────────────┘  └────────────────────────────┘   │  │
│  └──────────┬──────────────┬──────────────────┬──────────────┘  │
│             │              │                  │                  │
│  ┌──────────▼──────┐  ┌───▼──────────┐  ┌───▼──────────────┐  │
│  │  Providers      │  │  Tool        │  │   Memory         │  │
│  │  (re-export)    │  │  Registry    │  │   Manager        │  │
│  │                 │  │              │  │                  │  │
│  │ ┌─────────────┐ │  │ ┌──────────┐ │  │                   │  │
│  │ │  OpenAI     │ │  │ │Filesystem│ │  │  Long-term       │  │
│  │ │  Compatible │ │  │ │Shell     │ │  │  Memory System   │  │
│  │ │  Provider   │ │  │ │WebSearch │ │  │  (Scenario-based)│  │
│  │ ├─────────────┤ │  │ │WebFetch  │ │  └─────────────────┘  │
│  │ │  Gemini     │ │  │ │Spawn    │ │                       │
│  │ │  Provider   │ │  │ │SpawnPar.│ │  ┌─────────────────┐  │
│  │ │             │ │  │ │Message  │ │  │  EventStore     │  │
│  │ ├─────────────┤ │  │ │Cron     │ │  │  (SQLite Backend)│  │
│  │ │  Copilot    │ │  │ │MCP Tools│ │  │                 │  │
│  │ │  Provider   │ │  │ │Memory   │ │  │  session_events │  │
│  │ └─────────────┘ │  │ └──────────┘ │  │  memory_metadata│  │
│  │                 │  │              │  └─────────────────┘  │
│  │                 │  │              │                       │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  kernel (Pure Function Execution Core)                  ││
│  │  ├── executor.rs: AgentExecutor, ToolExecutor          ││
│  │  ├── stream.rs: StreamEvent streaming output           ││
│  │  └── context.rs: RuntimeContext, KernelConfig          ││
│  └─────────────────────────────────────────────────────────┘│
│                                                             │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  subagents (Subagent System)                            ││
│  │  ├── manager.rs: spawn_subagent, TaskSpec              ││
│  │  ├── tracker.rs: SubagentTracker, parallel coordination││
│  │  └── runner.rs: ModelResolver                          ││
│  └─────────────────────────────────────────────────────────┘│
│                                                             │
│  │                │                                            │
│  │  Router Actor  │   ┌───────────────────────────────────┐   │
│  │  Session Actor │   │   Pipeline Hooks                  │   │
│  │  Outbound Actor│   │   ~/.gasket/hooks/               │   │
│  └───────┬────────┘   │   BeforeRequest.sh                │   │
│          │            │   AfterResponse.sh                │   │
│  ┌───────▼──────────────────────────┐  └──────────────────┘   │
│  │        Channel Manager           │                         │
│  │  ┌──────┐ ┌───────┐ ┌────────┐  │                         │
│  │  │Tele- │ │Discord│ │ Slack  │  │  ┌───────────────────┐  │
│  │  │gram  │ │       │ │        │  │  │   Config Loader   │  │
│  │  ├──────┤ ├───────┤ ├────────┤  │  │   (YAML → Struct) │  │
│  │  │Feishu│ │       │ │DingTalk│  │  └───────────────────┘  │
│  │  ├──────┤ ├───────┤ ├────────┤  │                         │
│  │  │WeCom │ │WebSock│ │  CLI   │  │  ┌───────────────────┐  │
│  │  └──────┘ └───────┘ └────────┘  │  │   Skills Loader   │  │
│  └─────────────────────────────────┘  │   (MD → Context)  │  │
│                                                               │
│  ┌───────────────┐  ┌────────────────┐  ┌──────────────────┐ │
│  │  Heartbeat    │  │  Cron Service  │  │  MCP Client      │ │
│  │  Service      │  │  (file-driven: │  │  (JSON-RPC 2.0)  │ │
│  │               │  │   ~/.gasket/   │  │                  │ │
│  │               │  │   cron/*.md)   │  │                  │ │
│  └───────────────┘  └────────────────┘  └──────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Vault (Sensitive Data Isolation)           │  │
│  │              (engine internal module)                   │  │
│  │                                                         │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │  │
│  │  │ VaultStore  │  │ VaultInjector│  │  VaultCrypto  │  │  │
│  │  │ (JSON Store)│  │ (Runtime Inj)│  │  (XChaCha20)  │  │  │
│  │  └─────────────┘  └──────────────┘  └───────────────┘  │  │
│  │                                                         │  │
│  │  Placeholder syntax: {{vault:key}}                      │  │
│  │  Log redaction: redact_secrets()                        │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Search (Search Types Module)               │  │
│  │              (re-export from storage with local-embedding)           │  │
│  │                                                         │  │
│  │  SearchQuery: BooleanQuery, FuzzyQuery, DateRange       │  │
│  │  SearchResult: HighlightedText                          │  │
│  │  TextEmbedder, cosine_similarity                        │  │
│  │  Note: Advanced Tantivy full-text search migrated       │  │
│  │        to standalone tantivy service                │  │
│  └─────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘

                    ┌─────────────────────┐
                    │   External LLM APIs  │
                    │  OpenAI / Anthropic  │
                    │  DeepSeek / Gemini   │
                    │  Ollama / Copilot    │
                    └─────────────────────┘
```

### Core Design Principles

| Principle | Implementation |
|-----------|----------------|
| **AgentContext enum** | Zero-cost enum dispatch instead of Option<T> pattern, PersistentContext variant (full deps) and Stateless variant (no persistence) |
| **Kernel pure function design** | `kernel::execute()` and `kernel::execute_streaming()` with no side effects, clear input/output |
| **Session state management** | `AgentSession` wraps kernel, manages session state, prompt loading, hook registration |
| **Pipeline Hook extension** | Five execution points (BeforeRequest, AfterHistory, BeforeLLM, AfterToolCall, AfterResponse) with sequential/parallel strategies |
| **Feature flag compilation** | Communication channels compiled via Cargo feature flags, enable on demand |
| **No in-memory cache** | Session reads/writes SQLite directly, leverages SQLite page cache to avoid consistency issues |
| **Vault sensitive data isolation** | Sensitive data completely isolated from LLM-accessible storage, injected only at runtime, supports encrypted storage |
| **Modular Skills system** | Independent skills/ module, supports Markdown + YAML frontmatter format, progressive loading |
| **File-driven Cron** | Cron jobs stored in ~/.gasket/cron/*.md, notify watches for hot reload, no SQLite persistence |
| **Crate separation** | Core types, providers, storage, channels split into independent crates |

---

## Module Dependencies

```
engine
    │
    ├── re-exports from types
    │       └── Tool trait, events (ChannelType, SessionKey, InboundMessage, etc.)
    │       └── SessionEvent, EventType, Session (event sourcing types)
    │
    ├── re-exports from providers
    │       └── LlmProvider trait, ChatRequest, ChatResponse, etc.
    │
    ├── re-exports from storage (as memory module)
    │       └── SqliteStore, EventStore, StoreError, MemoryStore
    │       └── memory submodule (MetadataStore, EmbeddingStore, etc.)
    │
    ├── session/ (Session management layer)
    │       └── AgentSession (formerly AgentLoop), AgentContext, ContextCompactor
    │       └── MemoryManager, MemoryProvider trait
    │
    ├── kernel/ (Pure function execution core)
    │       └── AgentExecutor, ToolExecutor, execute(), execute_streaming()
    │
    ├── subagents/ (Subagent system)
    │       └── spawn_subagent, SubagentTracker
    │
    ├── optional: channels (feature flags)
    │       └── Telegram, Discord, Slack, Feishu, DingTalk, WeCom, WebSocket
    │
    └── optional: providers (feature flags)
            └── Gemini, Copilot
```

---

## Key Components

### AgentSession (formerly AgentLoop)

`AgentSession` is the core session management structure:

```rust
pub struct AgentSession {
    runtime_ctx: RuntimeContext,    // Kernel execution context
    context: AgentContext,          // Persistent/stateless context
    config: AgentConfig,            // Agent configuration
    workspace: PathBuf,             // Workspace path
    system_prompt: String,          // System prompt
    skills_context: Option<String>, // Skills context
    hooks: Arc<HookRegistry>,       // Hook registry
    history_config: gasket_storage::HistoryConfig, // History configuration
    compactor: Option<Arc<ContextCompactor>>, // Context compactor
    memory_manager: Option<Arc<MemoryManager>>, // Memory manager
    indexing_service: Option<Arc<IndexingService>>, // Indexing service
    pricing: Option<ModelPricing>,  // Optional pricing for cost calculation
    pending_done: tokio_util::task::TaskTracker, // Graceful shutdown tracker
}
```

**Key methods:**
- `process_direct()` — Process message and return response
- `process_direct_streaming_with_channel()` — Streaming processing

### Kernel Execution Core

Pure function design with no side effects:

```rust
/// Pure function: Execute LLM conversation loop
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError>;

/// Pure function: Streaming LLM conversation loop
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError>;
```

### AgentContext Enum

Zero-cost enum dispatch that replaces `Option<T>` pattern at compile time:

```rust
pub enum AgentContext {
    Persistent(PersistentContext),
    Stateless,
}
```

```rust
pub struct PersistentContext {
    pub event_store: Arc<EventStore>,
    pub sqlite_store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}
```

Key methods on AgentContext:
- `persistent(event_store, sqlite_store) -> Self` — create persistent variant
- `is_persistent(&self) -> bool` — check variant at runtime
- `load_session(&self, key) -> Session` — load session from event store
- `save_event(&self, event) -> Result` — append event to event store
- `get_events_after_watermark(&self, key, watermark) -> Vec<SessionEvent>` — retrieve events after watermark
- `recall_history(&self, key, embedding, top_k) -> Vec<String>` — semantic recall
- `clear_session(&self, key) -> Result` — clear session data

| Variant | Purpose |
|---------|---------|
| `Persistent(PersistentContext)` | Main agent, full event sourcing with SQLite |
| `Stateless` | Subagent, no persistence, pure computation |

### Event Sourcing Architecture

The session system uses Event Sourcing to store immutable facts about conversation history, enabling branching, versioning, and full audit trails.

**SessionEvent** - Immutable event records with UUID v7 (time-ordered):
```rust
pub struct SessionEvent {
    pub id: Uuid,                    // UUID v7 (time-ordered, sortable)
    pub parent_id: Option<Uuid>,     // For branching/version control
    pub event_type: EventType,
    pub payload: JsonValue,
    pub metadata: EventMetadata,
}
```

**EventType** - Core event variants:
```rust
pub enum EventType {
    UserMessage,      // User input message
    AssistantMessage, // LLM response
    ToolCall,         // Tool invocation request
    ToolResult,       // Tool execution result
    Summary,          // Context summarization
    // Note: Branching is not currently implemented
}
```

**Session Aggregate** - Aggregate root managing branch state:
```rust
pub struct Session {
    pub key: String,
    pub metadata: SessionMetadata,
}
```

**Branching Support** - Version control for conversations:
- `parent_id` links events in a chain (linked list structure)
- `branches` HashMap tracks multiple branch heads per session
- Each branch is an independent event chain from a common ancestor
- Enables time-travel, parallel exploration, and merge operations

```
Session (Aggregate Root)
  ├── branches: HashMap<branch_name, event_id>
  └── metadata: SessionMetadata

SessionEvent (Immutable Fact)
  ├── id: Uuid (v7 time-ordered)
  ├── parent_id: Option<Uuid> (for branching)
  ├── event_type: EventType
  └── metadata: EventMetadata
```

### Hook System

```rust
pub enum HookPoint {
    BeforeRequest,  // Sequential, can modify/abort
    AfterHistory,   // Sequential, can modify
    BeforeLLM,      // Sequential, last chance to modify
    AfterToolCall,  // Parallel, read-only
    AfterResponse,  // Parallel, read-only
}
```

### Feature Flags

| Crate | Flag | Purpose |
|-------|------|---------|
| engine | `local-embedding` | ONNX embedding via fastembed |
| engine | `telegram` | Telegram channel |
| engine | `discord` | Discord channel |
| engine | `slack` | Slack channel |
| engine | - | - |
| engine | `feishu` | Feishu channel |
| engine | `dingtalk` | DingTalk channel |
| engine | `wecom` | WeCom channel |
| engine | `websocket` | WebSocket channel |
| engine | `provider-gemini` | Google Gemini provider |
| engine | `provider-copilot` | GitHub Copilot provider |
| storage | `local-embedding` | fastembed ONNX embedding (~20MB) |
| cli | `full` | All features combined |
| cli | `telemetry` | OpenTelemetry support |

### Actor Model

| Actor | Responsibility | Characteristics |
|-------|----------------|-----------------|
| Router | Distributes messages to Session Actors by SessionKey | Single task, HashMap routing table |
| Session | Processes single session messages serially | One per session, idle timeout self-destruction |
| Outbound | HTTP/WebSocket sending | Single task, fire-and-forget sending |

---

## Extension Crates

| Crate | Purpose | Dependencies |
|-------|---------|--------------|
| `types` | Shared type definitions, minimal deps | None |
| `providers` | LLM provider implementations | types, async-trait |
| `storage` | SQLite storage + embedding + memory system | types, sqlx, fastembed |
| `channels` | Communication channels | teloxide, serenity, etc. |
| `sandbox` | Sandbox execution | System process management |
| `tantivy` | Full-text search MCP server | tantivy |
