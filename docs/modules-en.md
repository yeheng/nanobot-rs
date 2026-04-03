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
    async fn execute(&self, args: Value) -> ToolResult;
}
```

### Built-in Tool List

| Tool | Category | Description |
|------|----------|-------------|
| `read_file` | filesystem | Read file content |
| `write_file` | filesystem | Write file |
| `edit_file` | filesystem | Edit file (search/replace) |
| `list_dir` | filesystem | List directory contents |
| `exec` | system | Execute shell command (with timeout + command_policy) |
| `spawn` | system | Create subagent to execute task |
| `spawn_parallel` | system | Execute multiple tasks in parallel with subagents |
| `web_fetch` | web | HTTP GET request |
| `web_search` | web | Web search (Brave/Tavily/Exa/Firecrawl) |
| `message` | communication | Send message through Bus to channel |
| `cron` | system | Manage scheduled tasks (CRUD) |
| `memory_search` | memory | Search structured memories (FTS5) |
| `history_search` | memory | Search session history |
| MCP tools | mcp | Dynamic tools provided by MCP servers |

### Helper Modules

| Module | Description |
|------|-------------|
| `registry.rs` | `ToolRegistry` — Tool registry with semantic routing support |
| `base.rs` | Re-exports `Tool` trait, `ToolContext`, `ToolError` from types crate |

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
| Email | `email` | IMAP Polling + SMTP | Email send/receive |
| DingTalk | `dingtalk` | HTTP Webhook (axum) | DingTalk callback |
| WeCom | `wecom` | HTTP Webhook (axum) | WeCom callback |
| WebSocket | `webhook` | WebSocket (axum) | Real-time bidirectional communication |

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

## 5. bus/ — Message Bus (Actor Model)

### Module Structure

| File | Responsibility |
|------|----------------|
| `events.rs` | Event type definitions: `ChannelType`, `SessionKey`, `InboundMessage`, `OutboundMessage`, `MediaAttachment`, `SessionEvent`, `EventType`, `Session` |
| `actors.rs` | Three Actors: `run_router_actor`, `run_session_actor`, `run_outbound_actor` |
| `queue.rs` | Message queue encapsulation |

### Actor Pipeline

```
Inbound → [Router Actor] → per-session channel → [Session Actor] → [Outbound Actor] → HTTP
```

- **Router Actor**: Owns routing table `HashMap<SessionKey, Sender>`, distributes by session, lazy creation/cleanup
- **Session Actor**: Processes single session messages serially, shares `Arc<AgentLoop>`, self-destructs on idle timeout
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

## 7. memory/ — Storage Abstraction Layer

### MemoryStore Trait

```rust
#[async_trait]
trait MemoryStore: Send + Sync {
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;
    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>>;
    async fn delete(&self, id: &str) -> Result<bool>;
    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>>;
}
```

### SqliteStore Implementation

- Uses `sqlx::SqlitePool` native async I/O
- FTS5 full-text search support
- Submodules: `memories.rs` (FTS5), `session.rs` (session persistence), `kv.rs` (key-value store), `cron.rs` (scheduled tasks)

---

## 8. session/ — Session Management (Event Sourcing)

> **Note**: Event sourcing types defined in `types` crate (`SessionEvent`, `EventType`, `Session`), persistence in `storage` crate (`EventStore`).

### Core Types (from types crate)

| Type | Description |
|------|-------------|
| `Session` | Aggregate root with metadata (created_at, updated_at, total_events) |
| `SessionEvent` | Immutable events with UUID v7, session_key, event_type, content, optional embedding |
| `EventType` | UserMessage, AssistantMessage, ToolCall, ToolResult, Summary |
| `SummaryType` | TimeWindow, Topic, Compression |
| `EventMetadata` | branch, tools_used, token_usage, content_token_len, extra |
| `SessionMetadata` | created_at, updated_at, last_consolidated_event, total_events, total_tokens |

### Architecture

- **Event Sourcing**: All messages stored as immutable events enabling full history reconstruction
- **EventStore** (storage crate): `append_event()`, `get_branch_history()`, `get_events_by_ids()`, `clear_session()`, `get_latest_summary()`
- **Pure SQLite**: No in-memory cache, reads directly from database, leverages SQLite page cache
- **History Processing**: `process_history()` with token budget, recent_keep, max_events configuration
- **Query System**: `HistoryQueryBuilder` with branch, time_range, event_types, semantic_query, tools filters

---

## 9. agent/ — Agent Core Engine

| File | Responsibility |
|------|----------------|
| `loop_.rs` | `AgentLoop` — Core processing loop, orchestrates all components |
| `executor.rs` | `ToolExecutor` — Tool call execution (supports parallel batch) |
| `executor_core.rs` | `AgentExecutor` — Core LLM execution loop with streaming support |
| `context.rs` | `AgentContext` enum — Zero-cost enum dispatch (Persistent/Stateless) |
| `compactor.rs` | `ContextCompactor` — Synchronous context compression (replaces SummarizationService) |
| `indexing.rs` | `IndexingService` — Semantic indexing service (decoupled from compaction) |
| `stream.rs` | `StreamEvent` enum — Streaming output events (Content, Reasoning, ToolStart/End, Done) |
| `stream_buffer.rs` | `BufferedEvents` — WebSocket message buffering for ordering |
| `subagent.rs` | `SubagentManager` + `SubagentTaskBuilder` — Builder pattern subagent management |
| `subagent_tracker.rs` | `SubagentTracker` — Parallel task coordination with cancellation |
| `memory.rs` | `MemoryStore` — Session memory store wrapping SqliteStore |
| `prompt.rs` | Bootstrap file loading, skills context, token truncation |
| `request.rs` | `RequestHandler` — Request building with retry logic |
| `skill_loader.rs` | Skill file loading from workspace and built-in directories |

### AgentContext Enum

Zero-cost enum dispatch replacing the previous trait-based approach:

```rust
pub enum AgentContext {
    Persistent(PersistentContext),
    Stateless,
}

pub struct PersistentContext {
    pub event_store: Arc<EventStore>,
    pub sqlite_store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}
```

| Variant | Purpose |
|---------|---------|
| `Persistent(PersistentContext)` | Main agent with full event sourcing |
| `Stateless` | Subagent with no persistence |

### Context Compaction

`ContextCompactor` performs synchronous context compression when history is evicted:

```rust
pub struct ContextCompactor { /* provider, event_store, model, token_budget, threshold */ }

impl ContextCompactor {
    pub fn new(provider, event_store, model, token_budget) -> Self;
    pub fn with_summarization_prompt(self, prompt) -> Self;
    pub fn with_threshold(self, threshold: f32) -> Self;
    pub async fn compact(&self, session_key, evicted_events, vault_values) -> Result<Option<String>>;
}
```

When history messages exceed the token budget, the compactor calls the LLM to generate a summary and persists it as a Summary event.

### SubagentManager API

Builder pattern for flexible task creation:

```rust
let task_id = manager
    .task("sub-1", "Execute task")
    .with_provider(provider)
    .with_config(config)
    .with_system_prompt("Custom prompt".to_string())
    .with_streaming(event_tx)
    .with_session_key(session_key)
    .with_cancellation_token(token)
    .with_hooks(hooks)
    .spawn(result_tx)
    .await?;
```

---

## 10. config/ — Configuration Management

| File | Responsibility |
|------|----------------|
| `mod.rs` | Configuration module exports |
| `app_config.rs` | Main `Config` struct, `ConfigLoader`, `ModelConfig`, `ModelProfile`, `ModelRegistry`, `ProviderConfig`, `ProviderRegistry`, `ProviderType` |
| `tools.rs` | `ToolsConfig`, `ExecToolConfig` (command policy), `WebToolsConfig` (search/fetch/proxy), `SandboxConfig`, `CommandPolicyConfig`, `ResourceLimitsConfig`, `EmbeddingConfig` |

- Config file at `~/.gasket/config.yaml`
- Compatible with Python gasket configuration format

---

## 11. vault/ — Sensitive Data Isolation Module

> Detailed usage guide in [vault-guide.md](vault-guide.md)

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

## 12. search/ — Search & Embedding

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

## 13. Other Modules

| Module | Description |
|------|-------------|
| `cron/` | `CronService` + `CronJob` — Scheduled task service with SQLite persistence |
| `heartbeat/` | `HeartbeatService` — Reads HEARTBEAT.md, triggers periodic proactive tasks |
| `skills/` | Skills system — `SkillsLoader`, `SkillsRegistry`, `Skill`, `SkillMetadata` (see Section 14) |
| `bus_adapter.rs` | `EngineHandler` — Bridges engine to bus actor system |
| `error.rs` | Unified error types (AgentError, ProviderError, ChannelError, PipelineError, ConfigValidationError) |
| `token_tracker.rs` | Token counting, cost calculation, session stats tracking |

---

## 14. skills/ — Skills System

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
