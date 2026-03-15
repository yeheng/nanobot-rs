# Module Design

> Nanobot-RS Module Responsibilities and Interface Design

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
| `registry.rs` | `ToolRegistry` — Tool registry, manages all available tools |
| `sandbox.rs` | `SandboxProvider` — Sandbox constraints (directory restrictions) |
| `resource_limits.rs` | Resource limits (file size, output length, etc.) |
| `command_policy.rs` | Shell command policy (whitelist/blacklist) |

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
│  (nanobot)  │                     │  (External proc) │
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
| `events.rs` | Event type definitions: `ChannelType`, `SessionKey`, `InboundMessage`, `OutboundMessage`, `MediaAttachment` |
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

## 6. hooks/ — External Shell Hook System

```
Rust → stdin (JSON) → Shell Script → stdout (JSON) → Rust
                        stderr → tracing::debug!
```

- Scripts located in `~/.nanobot/hooks/`
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

## 8. session/ — Session Management

**SessionManager**: Pure SQLite backend, no in-memory cache

- Reads directly from SQLite every time, eliminates cache consistency issues
- Leverages SQLite page cache to ensure read performance
- Supports legacy JSON blob session automatic migration
- Messages stored independently (O(1) append)

---

## 9. agent/ — Agent Core Engine

| File | Responsibility |
|------|----------------|
| `loop_.rs` | `AgentLoop` — Core processing loop, orchestrates all components |
| `executor.rs` | `ToolExecutor` — Tool call execution (supports parallel batch execution) |
| `history_processor.rs` | Token-aware history truncation (tiktoken-rs BPE encoding) |
| `prompt.rs` | System prompt loading (bootstrap files + skills + token truncation protection) |
| `summarization.rs` | `SummarizationService` + `ContextCompressionHook` — LLM summarization |
| `stream.rs` | Stream output accumulator |
| `request.rs` | Request building and retry logic |
| `memory.rs` | Agent workspace memory management |
| `skill_loader.rs` | Skill file loader (Markdown + YAML frontmatter) |
| `subagent.rs` | Subagent management (`submit()` async + `submit_and_wait()` sync + `submit_tracked()` tracked + `submit_tracked_streaming()` streaming) |

### ContextCompressionHook

Extensible context compression interface, decouples compression strategy from Agent loop:

```rust
#[async_trait]
trait ContextCompressionHook: Send + Sync {
    async fn compress(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
    ) -> Result<Option<String>>;
}
```

Current implementation `SummarizationService`: When history messages are evicted, calls LLM to generate summary and persists to SQLite.

> **Note**: `ContextCompressionHook` has been simplified to `SummarizationService`'s `compress()` method, no longer as an independent trait. `AgentContext::compress_context()` directly calls this method.

---

## 10. config/ — Configuration Management

- `loader.rs` — Configuration file loading (`~/.nanobot/config.yaml`)
- `schema.rs` — Configuration structure definitions (providers, agents, channels, tools, etc.)
- `provider.rs` — Provider configuration definitions
- `agent.rs` — Agent configuration definitions
- `channel.rs` — Channel configuration definitions
- Compatible with Python nanobot configuration format

---

## 11. vault/ — Sensitive Data Isolation Module

> Detailed usage guide in [vault-guide.md](vault-guide.md)

### Core Components

| File | Responsibility |
|------|----------------|
| `store.rs` | `VaultStore` — JSON file storage, supports encryption |
| `injector.rs` | `VaultInjector` — Runtime placeholder replacement |
| `scanner.rs` | Placeholder scanning and parsing (`{{vault:key}}`) |
| `crypto.rs` | `VaultCrypto` — AES-256-GCM encryption |
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

## 12. search/ — Search Type Definitions

### Core Types

```rust
// Search query
pub enum SearchQuery {
    Boolean(BooleanQuery),
    Fuzzy(FuzzyQuery),
    DateRange(DateRange),
}

// Search result
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub highlights: Vec<HighlightedText>,
}

pub struct HighlightedText {
    pub field: String,
    pub text: String,
    pub highlights: Vec<(usize, usize)>, // Highlight ranges
}
```

> **Note**: Advanced Tantivy full-text search has been migrated to the standalone `tantivy-mcp` MCP server.

---

## 13. Other Modules

| Module | Description |
|------|-------------|
| `cron/` | Scheduled task service, checks due tasks every 60 seconds |
| `heartbeat/` | Heartbeat service, reads HEARTBEAT.md and triggers periodically |
| `crypto/` | Cryptographic tools (message encryption/decryption required by some channels like WeCom) |
| `skills/` | Skills system (see below) |
| `webhook/` | Webhook HTTP server (axum) |
| `workspace/` | Workspace template files (copied during initialization) |
| `error.rs` | Unified error type definitions (AgentError, ProviderError, McpError, ChannelError, PipelineError) |
| `token_tracker.rs` | Token counting and tracking |

---

## 14. skills/ — Skills System

### Module Structure

| File | Responsibility |
|------|----------------|
| `loader.rs` | `SkillsLoader` — Load skills from Markdown files |
| `registry.rs` | `SkillsRegistry` — Skills registry management |
| `skill.rs` | `Skill` — Skill definition structure |
| `metadata.rs` | `SkillMetadata` — Skill metadata (dependencies, tags, etc.) |

### Skill File Format

```markdown
---
name: my_skill
description: A sample skill
dependencies:
  binaries: ["node", "npm"]
  env_vars: ["API_KEY"]
tags: ["automation", "web"]
always_load: false
---

# My Skill

Detailed description and usage of the skill...
```

### Loading Modes

- **always_load: true** — Auto-load at startup
- **always_load: false** — Load on demand
