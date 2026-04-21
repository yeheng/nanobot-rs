# Module Design

> Gasket-RS Module Responsibilities and Interface Design

---

## 1. providers/ — LLM Provider Abstraction Layer

### Core Trait

```rust
#[async_trait]
trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream>;
}
```

### Provider Implementations

```
              ┌──────────────────────────┐
              │  trait LlmProvider       │
              │  ├── name()             │
              │  ├── default_model()    │
              │  ├── chat(ChatRequest)  │
              │  └── chat_stream()      │
              └──────────┬───────────────┘
                         │
         ┌───────────────┼───────────────┐
         │               │               │
┌────────▼──────┐ ┌──────▼──────┐ ┌──────▼───────┐
│OpenAI         │ │  Gemini     │ │  Copilot     │
│Compatible     │ │  Provider   │ │  Provider    │
│Provider       │ │             │ │              │
│               │ └─────────────┘ └──────────────┘
│ from_name():  │
│ ┌───────────┐ │
│ │ openai    │ │
│ │ openrouter│ │
│ │ deepseek  │ │
│ │ anthropic │ │
│ │ zhipu     │ │
│ │ dashscope │ │
│ │ moonshot  │ │
│ │ minimax   │ │
│ │ ollama    │ │
│ │ litellm   │ │
│ └───────────┘ │
└───────────────┘
```

- **OpenAICompatibleProvider**: Configured via `PROVIDER_DEFAULTS` table, adding a new provider only requires adding a row of data, no code needed
- **GeminiProvider**: Google Gemini API (non-OpenAI compatible format)
- **CopilotProvider**: GitHub Copilot API (with OAuth authentication flow)

**ModelSpec parsing format**: `provider_id/model_id` or `model_id`

| Input | provider | model |
|------|----------|-------|
| `deepseek/deepseek-chat` | `deepseek` | `deepseek-chat` |
| `anthropic/claude-4.5-sonnet` | `anthropic` | `claude-4.5-sonnet` |
| `gpt-4o` | `None` (use default) | `gpt-4o` |

---

## 2. tools/ — Tool System

### Core Trait

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;
}
```

### Built-in Tool List

| Tool | Category | Description |
|------|----------|-------------|
| `read_file` | filesystem | Read file content |
| `write_file` | filesystem | Write file |
| `edit_file` | filesystem | Edit file (search/replace) |
| `list_dir` | filesystem | List directory contents |
| `exec` | system | Execute shell command (with timeout + policy: allowlist/denylist) |
| `spawn` | system | Create subagent to execute task |
| `spawn_parallel` | system | Execute multiple tasks in parallel with subagents |
| `web_fetch` | web | HTTP GET request |
| `web_search` | web | Web search (Brave/Tavily/Exa/Firecrawl) |
| `MessageTool` | communication | Send message through Broker to channel |
| `cron` | system | Manage scheduled tasks (CRUD) |
| `memory_search` | memory | Search structured memories via SQLite MetadataStore |
| `memorize` | memory | Write structured long-term memories |
| MCP tools | mcp | Dynamic tools provided by MCP servers |
| Plugin tools | plugin | External script tools loaded from `~/.gasket/plugins/` |

### Helper Modules

| Module | Description |
|------|-------------|
| `registry.rs` | `ToolRegistry` — Tool registry with semantic routing support |
| `base.rs` | Re-exports `Tool` trait, `ToolContext`, `ToolError` from types crate |
| `wiki_decay.rs` | `WikiDecayTool` — Wiki page decay tool (formerly memory_decay) |
| `wiki_refresh.rs` | `WikiRefreshTool` — Wiki index refresh tool (formerly memory_refresh) |
| `wiki_tools.rs` | `WikiReadTool`, `WikiSearchTool`, `WikiWriteTool` — Wiki read/write/search tools |

---

## 2.5. plugin/ — External Plugin System

> Located at `engine/src/plugin/`

The plugin system loads external scripts via YAML manifests and exposes them as native tools.

### Module Structure

| File | Responsibility |
|------|----------------|
| `mod.rs` | `PluginTool` — Tool trait implementation for external scripts |
| `manifest.rs` | `PluginManifest`, `PluginProtocol`, `RuntimeConfig`, `Permission` |
| `rpc.rs` | JSON-RPC 2.0 message types (`RpcMessage`, `RpcRequest`, `RpcResponse`) and line codec |
| `runner/simple.rs` | One-shot stdin/stdout runner for Simple protocol |
| `runner/jsonrpc.rs` | Bidirectional JSON-RPC runner |
| `runner/daemon.rs` | `JsonRpcDaemon` — persistent JSON-RPC process with request multiplexing |
| `dispatcher/mod.rs` | `RpcDispatcher` — routes RPC calls with permission enforcement |
| `dispatcher/llm_chat.rs` | Handler for `llm/chat` |
| `dispatcher/memory_search.rs` | Handler for `memory/search` |
| `dispatcher/memory_write.rs` | Handler for `memory/write` |
| `dispatcher/memory_decay.rs` | Handler for `memory/decay` |
| `dispatcher/subagent.rs` | Handler for `subagent/spawn` |

### Protocols

- **Simple**: One-shot JSON input/output via stdin/stdout
- **JsonRpc**: Bidirectional JSON-RPC 2.0 with callback methods (`llm/chat`, `memory/search`, etc.)

### Permissions (Default Deny)

| Permission | RPC Method |
|------------|------------|
| `LlmChat` | `llm/chat` |
| `MemorySearch` | `memory/search` |
| `MemoryWrite` | `memory/write` |
| `MemoryDecay` | `memory/decay` |
| `SubagentSpawn` | `subagent/spawn` |

---

## 3. channels/ — Communication Channels

### Core Trait

```rust
#[async_trait]
trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&mut self) -> Result<()>;  // Start receiving messages
    async fn stop(&mut self) -> Result<()>;   // Stop
    async fn graceful_shutdown(&mut self) -> Result<()>;
}
```

> Channel is **inbound-only**: receives external messages and pushes to internal Bus. All **outbound** sending is handled by Outbound Actor through `send_outbound()` function routing by channel type.

### Channel List

| Channel | Feature Flag | Transport Protocol | Description |
|------|-------------|----------|------|
| Telegram | `telegram` | Long Polling (teloxide) | Telegram Bot API |
| Discord | `discord` | WebSocket (serenity) | Discord Gateway |
| Slack | `slack` | WebSocket (tungstenite) | Slack Socket Mode |
| Feishu | `feishu` | HTTP Webhook (axum) | Feishu event subscription |
| DingTalk | `dingtalk` | HTTP Webhook (axum) | DingTalk callback |
| WeCom | `wecom` | HTTP Webhook (axum) | WeCom callback |
| WebSocket | `websocket` | WebSocket (axum) | Real-time bidirectional communication |

### Middleware Layer

| Component | Description |
|------|-------------|
| `SimpleAuthChecker` | Whitelist-based sender authentication |
| `SimpleRateLimiter` | Simple rate limiting |
| `InboundSender` | Encapsulates inbound message sending logic |
| `log_inbound` | Inbound message logging |

---

## 4. mcp/ — Model Context Protocol

```
┌─────────────┐    JSON-RPC 2.0     ┌──────────────────┐
│  MCP Client │◄───── stdio ───────▶│  MCP Server      │
│  (gasket)  │                     │  (External proc) │
│             │                     │                  │
│  initialize │────────────────────▶│  Return tool list│
│  tools/list │────────────────────▶│  Return tool def │
│  tools/call │────────────────────▶│  Execute & return│
└─────────────┘                     └──────────────────┘
```

### Submodule Structure

| File | Responsibility |
|------|----------------|
| `client.rs` | `McpClient` — JSON-RPC 2.0 over stdio communication |
| `manager.rs` | `McpManager` — Manages multiple MCP server lifecycles |
| `tool.rs` | `McpToolBridge` — Adapts MCP tools to `trait Tool` |
| `types.rs` | `McpServerConfig`, `McpTool` and other type definitions |

---

## 5. broker/ — Message Bus (Actor Model)

### Module Structure

| File | Responsibility |
|------|----------------|
| `events.rs` | Re-exported from `types`: `ChannelType`, `SessionKey`, `InboundMessage`, `OutboundMessage`, `MediaAttachment` |
| `actors.rs` | Three Actors: `run_router_actor`, `run_session_actor`, `run_outbound_actor` |
| `queue.rs` | Message queue encapsulation |

### Actor Pipeline

```
Inbound → [Router Actor] → per-session channel → [Session Actor] → [Outbound Actor] → HTTP
```

- **Router Actor**: Owns routing table `HashMap<SessionKey, Sender>`, distributes by session, lazy creation/cleanup
- **Session Actor**: Processes single session messages serially, shares `Arc<AgentSession>`, self-destructs on idle timeout
- **Outbound Actor**: Dedicated network sending, isolates external API latency

---

## 6. hooks/ — Agent Pipeline Lifecycle Hook System

Unified pipeline extension mechanism with five execution points and sequential/parallel strategies.

### Hook Points

| Hook Point | Timing | Strategy | Description |
|------------|--------|----------|-------------|
| `BeforeRequest` | Before request processed | Sequential | Can modify input, can abort |
| `AfterHistory` | After history loaded | Sequential | Can add context |
| `BeforeLLM` | Before sending to LLM | Sequential | Last chance to modify |
| `AfterToolCall` | After tool call completes | Parallel | Read-only, fire-and-forget |
| `AfterResponse` | After response generated | Parallel | Audit/alert |

### Core Components

| Component | Responsibility |
|-----------|----------------|
| `HookRegistry` | Hook registry, manages all hooks by point |
| `PipelineHook` | Hook trait with `name()`, `point()`, `run()`, `run_parallel()` |
| `HookBuilder` | Builder for creating HookRegistry |
| `HookContext<M>` | Generic context with session_key, messages, user_input, response |
| `ExternalShellHook` | Shell script hook wrapper |
| `HistoryRecallHook` | Semantic history recall (feature: local-embedding) |
| `VaultHook` | Vault secret injection at BeforeLLM |

### External Shell Hooks

```
Rust → stdin (JSON) → Shell Script → stdout (JSON) → Rust
                        stderr → tracing::debug!
```

- Scripts located in `~/.gasket/hooks/`
- `pre_request.sh` — Request preprocessing (can modify or abort input)
- `post_response.sh` — Post-response processing (audit/alert)
- 2 second timeout, 1 MB stdout limit, non-blocking `tokio::process::Command`

---

## 7. storage/ — Storage Abstraction Layer

> **Note**: Implementation re-exported from `storage` crate.

### Core Components

| Component | Description |
|-----------|-------------|
| `EventStore` | Event sourcing storage (session_events table) |
| `SqliteStore` | SQLite general storage (sessions, summaries, cron jobs, kv) |
| `processor` | `process_history()` — Token-budget-aware history processing |
| `query` | `HistoryQueryBuilder` — History query builder |
| `search/` | FTS5 full-text search types |
| `wiki/` | Wiki page storage (page_store, relation_store, source_store) |

### SqliteStore

- Uses `sqlx::SqlitePool` native async I/O
- WAL mode for concurrent reads
- Submodules: `fs.rs` (filesystem), `event_store.rs` (events), `wiki/` (knowledge base)

---

## 8. Event Sourcing

> **Note**: Event sourcing types defined in `types` crate (`SessionEvent`, `EventType`, `Session`), persistence in `storage` crate (`EventStore`).

### Core Types (from types crate)

| Type | Description |
|------|-------------|
| `Session` | Aggregate root with metadata (created_at, updated_at, total_events) |
| `SessionEvent` | Immutable events with UUID v7, session_key, event_type, content, optional embedding |
| `EventType` | UserMessage, AssistantMessage, ToolCall, ToolResult, Summary |
| `SummaryType` | TimeWindow, Topic, Compression |
| `EventMetadata` | tools_used, token_usage, content_token_len, extra |
| `SessionMetadata` | created_at, updated_at, last_consolidated_event, total_events, total_tokens |

### Architecture

- **Event Sourcing**: All messages stored as immutable events enabling full history reconstruction
- **EventStore** (storage crate): `append_event()`, `get_events_after_watermark()`, `get_events_by_ids()`, `clear_session()`, `get_latest_summary()`
- **Pure SQLite**: No in-memory cache, reads directly from database, leverages SQLite page cache
- **History Processing**: `process_history()` with token budget, recent_keep, max_events configuration
- **Query System**: `HistoryQueryBuilder` with time_range, event_types, semantic_query, tools filters

---

## 9. session/ — Session Management

> **Note**: `engine/src/agent/` has been restructured into `kernel/` + `session/` + `subagents/`

| File/Directory | Responsibility |
|------|----------------|
| `mod.rs` | `AgentSession` — Session management core, wraps kernel execution |
| `config.rs` | `AgentConfig` — Agent configuration with kernel conversion support |
| `context.rs` | `AgentContext` enum — Zero-cost enum dispatch (Persistent/Stateless) |
| `compactor.rs` | `ContextCompactor` — Context compression (based on token budget) |
| `prompt.rs` | Bootstrap file loading, skills context, token truncation |
| `store.rs` | `MemoryStore` — Memory store wrapper (exports MemoryStore only) |
| `history/` | Event sourcing history processing |

### history/ Submodule

| File | Responsibility |
|------|----------------|
| `builder.rs` | `HistoryBuilder` — History message builder |
| `coordinator.rs` | `HistoryCoordinator` — History loading coordinator |
| `indexing.rs` | `HistoryIndexingService` — Message indexing service |
| `mod.rs` | Module exports |

### AgentSession

`AgentSession` is the core session management structure that wraps kernel execution:

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
    indexing_service: Option<Arc<IndexingService>>, // Indexing service
    wiki: Option<WikiComponents>,  // Wiki knowledge system
    pricing: Option<ModelPricing>,  // Optional pricing for cost calculation
    pending_done: tokio_util::task::TaskTracker, // Graceful shutdown tracker
}
```

### AgentContext Enum

```rust
pub enum AgentContext {
    Persistent(PersistentContext),  // Main agent with full event sourcing
    Stateless,                      // Subagent with no persistence
}

pub struct PersistentContext {
    pub event_store: Arc<EventStore>,
    pub sqlite_store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
    pub coordinator: Option<Arc<HistoryCoordinator>>,
}
```

---

## 10. kernel/ — Pure Function Execution Core

| File | Responsibility |
|------|----------------|
| `mod.rs` | `execute()`, `execute_streaming()` — Pure function execution entry points |
| `executor.rs` | `AgentExecutor`, `ToolExecutor`, `ExecutionResult` — Executor implementations |
| `context.rs` | `RuntimeContext`, `KernelConfig` — Runtime context and configuration |
| `stream.rs` | `StreamEvent`, `BufferedEvents` — Streaming output events |
| `error.rs` | `KernelError` — Kernel error types |

### Pure Function Execution Interface

```rust
/// Execute LLM conversation loop
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError>;

/// Streaming LLM conversation loop
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError>;
```

---

## 11. subagents/ — Subagent System

| File | Responsibility |
|------|----------------|
| `manager.rs` | `spawn_subagent()`, `TaskSpec` — Pure function subagent spawning |
| `tracker.rs` | `SubagentTracker`, `TrackerError` — Parallel task coordination |
| `runner.rs` | `ModelResolver` — Subagent execution and model resolution |

### Spawning API

Subagent spawning uses a simple pure-function approach:

```rust
let task = TaskSpec::new("sub-1", "Execute task")
    .with_model("openrouter/anthropic/claude-4.5-sonnet")
    .with_system_prompt("Custom prompt".to_string());

let handle = spawn_subagent(
    provider,
    tools,
    workspace,
    task,
    Some(event_tx),
    result_tx,
    Some(token_tracker),
    cancellation_token,
);
```

### Subagent Result

```rust
pub struct SubagentResult {
    pub id: String,              // Subagent ID
    pub task: String,            // Task description
    pub response: SubagentResponse, // Execution result
    pub model: Option<String>,   // Model name used
}
```

---

## 12. config/ — Configuration Management

| File | Responsibility |
|------|----------------|
| `mod.rs` | Configuration module exports |
| `app_config.rs` | Main `Config` struct, `ConfigLoader`, `ModelConfig`, `ModelProfile`, `ModelRegistry`, `ProviderConfig`, `ProviderRegistry`, `ProviderType` |
| `tools.rs` | `ToolsConfig`, `ExecToolConfig` (command policy), `WebToolsConfig` (search/fetch/proxy), `SandboxConfig`, `CommandPolicyConfig`, `ResourceLimitsConfig`, `EmbeddingConfig` |

- Config file at `~/.gasket/config.yaml`
- Compatible with Python gasket configuration format

---

## 13. vault/ — Sensitive Data Isolation Module (inside engine)

> Detailed usage guide in [vault-guide.md](vault-guide.md)

Vault module is located at `engine/src/vault/`, not a separate crate.

### Core Components

| File | Responsibility |
|------|----------------|
| `store.rs` | `VaultStore` — JSON file storage, supports encryption |
| `injector.rs` | `VaultInjector` — Runtime placeholder replacement |
| `scanner.rs` | Placeholder scanning and parsing (`{{vault:key}}`) |
| `crypto.rs` | `VaultCrypto` — XChaCha20-Poly1305 encryption |
| `redaction.rs` | Log redaction functions (`redact_secrets`) |
| `error.rs` | `VaultError` error types |

### Design Principles

1. **Data structure isolation** — VaultStore completely independent from memory/history storage
2. **Runtime injection** — Sensitive data injected only at the last moment before sending to LLM
3. **Zero-trust design** — Sensitive data never persisted to LLM-accessible storage

### Placeholder Syntax

```
Use {{vault:api_key}} to access API
Password: {{vault:db_password}}
```

---

## 14. search/ — Search & Embedding

> **Note**: Search types re-exported from `storage` crate. Advanced Tantivy full-text search in standalone `tantivy` crate.

### Core Types

| Type | Description |
|------|-------------|
| `TextEmbedder` | ONNX-based text embedding via fastembed (feature: local-embedding) |
| `EmbeddingConfig` | Model name, cache dir, local model path configuration |
| `cosine_similarity()` | Calculate cosine similarity between two vectors |
| `top_k_similar()` | Get top-K most similar items from vector collection |
| `bytes_to_embedding()` | Convert byte slice to embedding vector |
| `embedding_to_bytes()` | Convert embedding vector to byte slice |

### Semantic Search Pipeline

1. `TextEmbedder::embed(text) -> Vec<f32>` — Generate embedding for query
2. `cosine_similarity(query, candidate) -> f32` — Score similarity
3. `top_k_similar(query, vectors, k) -> Vec<(f32, String)>` — Rank results

### History Query Builder

```rust
let results = HistoryQuery::builder("session-key")
    .branch("main")
    .time_range(start, end)
    .event_types(vec!["UserMessage".into()])
    .semantic_text("search query")
    .tools(vec!["exec".into()])
    .limit(10)
    .order(QueryOrder::ReverseChronological)
    .build();
```

---

## 15. Other Modules

| Module | Description |
|------|-------------|
| `cron/` | `CronService` + `CronJob` — Scheduled task service, file-driven |
| `heartbeat/` | `HeartbeatService` — Reads HEARTBEAT.md, triggers periodic proactive tasks |
| `skills/` | Skills system — `SkillsLoader`, `SkillsRegistry`, `Skill`, `SkillMetadata` (see Section 16) |
| `bus_adapter.rs` | `EngineHandler` — Bridges engine to bus actor system |
| `error.rs` | Unified error types (AgentError, ProviderError, ChannelError, PipelineError, ConfigValidationError) |
| `token_tracker.rs` | Token counting, cost calculation, session stats tracking |

---

## 16. skills/ — Skills System

### Module Structure

| File | Responsibility |
|------|----------------|
| `loader.rs` | `SkillsLoader` — Load skills from Markdown files |
| `registry.rs` | `SkillsRegistry` — Skills registry management |
| `skill.rs` | `Skill` — Skill definition structure |
| `metadata.rs` | `SkillMetadata` — Skill metadata (name, description, bins, env_vars, always, extra) |

### Skill File Format

```markdown
---
name: my_skill
description: A sample skill
bins: ["node", "npm"]
env_vars: ["API_KEY"]
always_load: false
---

# My Skill

Detailed description and usage of the skill...
```

### Loading Modes

- **always_load: true** — Auto-load at startup
- **always_load: false** — Load on demand

---

## 17. wiki/ — Wiki Knowledge System

> Located at `engine/src/wiki/`, three-layer architecture: Raw Sources → Compiled Wiki → Search Index.

### Module Structure

| File | Responsibility |
|------|----------------|
| `mod.rs` | Wiki module exports and re-exports |
| `page.rs` | `WikiPage`, `PageType`, `PageSummary`, `PageFilter`, `slugify()` |
| `store.rs` | `PageStore` — Wiki page CRUD |
| `index.rs` | `PageIndex` — Tantivy BM25 full-text search |
| `query/mod.rs` | `WikiQueryEngine`, `QueryResult`, `ScoredCandidate`, `SearchHit`, `Reranker`, `TantivyIndex` |
| `ingest/mod.rs` | Knowledge ingestion pipeline (parser, extractor, dedup) |
| `ingest/parser.rs` | `SourceParser`, `MarkdownParser`, `HtmlParser`, `PlainTextParser`, `ConversationParser` |
| `ingest/extractor.rs` | `KnowledgeExtractor`, `ExtractedItem`, `ExtractionResult` |
| `ingest/dedup.rs` | `SemanticDeduplicator`, `DedupResult` |
| `lint/mod.rs` | `WikiLinter`, `LintReport`, `FixReport` — Health checks (structural only) |
| `lint/structural.rs` | `StructuralIssue`, `StructuralIssueType`, `Severity`, `StructuralLintConfig` |
| `log.rs` | `WikiLog`, `LogEntry` — Operation logging |
| `lifecycle.rs` | `DecayReport`, `FrequencyManager` — Frequency decay and promotion management |

### Storage Wiki Module

> Located at `storage/src/wiki/`

| File | Responsibility |
|------|----------------|
| `mod.rs` | Wiki storage module exports |
| `page_store.rs` | `WikiPageStore`, `PageRow`, `DecayCandidate`, `WikiPageInput` |
| `tables.rs` | `create_wiki_tables()` — DDL table creation |
| `types.rs` | `Frequency`, `TokenBudget` — Core type definitions |
| `log_store.rs` | `WikiLogStore` — Log persistence |
| `relation_store.rs` | `WikiRelationStore` — Page relations |
| `source_store.rs` | `WikiSourceStore` — Source tracking |
