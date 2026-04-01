# Architecture Overview

> Gasket-RS System Architecture Overview

---

## Crate Structure

```
gasket-rs/                    (Cargo workspace)
├── engine/                   Core orchestration crate — Agent engine, tools, Hook system
│   └── src/
│       ├── agent/             Agent core engine (loop, executor, prompt, history, stream, summarization, subagent, context)
│       ├── bus/               Message bus (Actor model: Router/Session/Outbound)
│       ├── channels/          Communication channels re-export (from channels)
│       ├── config/            Configuration loading (YAML → Struct)
│       ├── cron/              Scheduled task service
│       ├── crypto/            Cryptographic tools
│       ├── heartbeat/         Heartbeat service
│       ├── hooks/             Pipeline Hook system (BeforeRequest, AfterResponse, etc.)
│       ├── memory/            Storage layer re-export (from storage)
│       ├── providers/         LLM provider re-export (from providers)
│       ├── session/           Session management (SQLite backend)
│       ├── skills/            Skills system (loader, registry, skill, metadata)
│       ├── tools/             Tool system (12 built-in tools)
│       ├── vault/             Sensitive data isolation re-export (from vault)
│       ├── webhook/           Webhook server
│       └── workspace/         Workspace template files
├── cli/                      CLI executable
│   └── src/
│       ├── main.rs            Command entry + Gateway launcher
│       ├── cli.rs             CLI interactive mode
│       ├── provider.rs        Provider factory
│       └── commands/          Subcommands (onboard, status, agent, gateway, channels, cron, vault)
├── types/                    Shared type definitions (Tool trait, events, etc.)
├── providers/                LLM provider implementations
├── storage/                  SQLite storage + embedding
├── vault/                    Vault sensitive data management
├── channels/                 Communication channel implementations
├── sandbox/                  Sandbox execution environment
├── bus/                      Message bus Actor implementation
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
│  │                      Agent Loop (Core Engine)               │  │
│  │  ┌────────────┐  ┌──────────────┐  ┌──────────────────┐   │  │
│  │  │  Prompt    │  │    Tool      │  │    History        │   │  │
│  │  │  Loader    │  │   Executor   │  │   Processor      │   │  │
│  │  └────────────┘  └──────────────┘  └──────────────────┘   │  │
│  │  ┌────────────────────┐  ┌────────────────────────────┐   │  │
│  │  │  Summarization     │  │      Hook Registry         │   │  │
│  │  │  Service           │  │  (BeforeRequest/AfterResp) │   │  │
│  │  └────────────────────┘  └────────────────────────────┘   │  │
│  └──────────┬──────────────┬──────────────────┬──────────────┘  │
│             │              │                  │                  │
│  ┌──────────▼──────┐  ┌───▼──────────┐  ┌───▼──────────────┐  │
│  │  Providers      │  │  Tool        │  │   Session        │  │
│  │  (re-export)    │  │  Registry    │  │   Manager        │  │
│  │                 │  │              │  │   (SQLite Backend)│  │
│  │ ┌─────────────┐ │  │ ┌──────────┐ │  │                   │  │
│  │ │  OpenAI     │ │  │ │Filesystem│ │  └─────────┬─────────┘  │
│  │ │  Compatible │ │  │ │Shell     │ │            │            │
│  │ │  Provider   │ │  │ │WebSearch │ │  ┌─────────▼─────────┐  │
│  │ ├─────────────┤ │  │ │WebFetch  │ │  │  Memory Store     │  │
│  │ │  Gemini     │ │  │ │Spawn    │ │  │  (re-export)      │  │
│  │ │  Provider   │ │  │ │Message  │ │  │  ┌─────────────┐  │  │
│  │ ├─────────────┤ │  │ │Cron     │ │  │  │ memories    │  │  │
│  │ │  Copilot    │ │  │ │MCP Tools│ │  │  │ sessions    │  │  │
│  │ │  Provider   │ │  │ │Memory   │ │  │  │ session_msg │  │  │
│  │ └─────────────┘ │  │ │ Search  │ │  │  │ kv_store    │  │  │
│  │                 │  │ │Sandbox  │ │  │  │ cron_jobs   │  │  │
│  └────────────────┘  │ └──────────┘ │  │  └─────────────┘  │  │
│                      │              │  └───────────────────┘  │
│  ┌────────────────┐  └──────────────┘                         │
│  │  Message Bus   │                                            │
│  │  (Actor Model) │                                            │
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
│  │  │Feishu│ │ Email │ │DingTalk│  │  └───────────────────┘  │
│  │  ├──────┤ ├───────┤ ├────────┤  │                         │
│  │  │WeCom │ │WebSock│ │  CLI   │  │  ┌───────────────────┐  │
│  │  └──────┘ └───────┘ └────────┘  │  │   Skills Loader   │  │
│  └─────────────────────────────────┘  │   (MD → Context)  │  │
│                                                               │
│  ┌───────────────┐  ┌────────────────┐  ┌──────────────────┐ │
│  │  Heartbeat    │  │  Cron Service  │  │  MCP Client      │ │
│  │  Service      │  │  (Scheduled)   │  │  (JSON-RPC 2.0)  │ │
│  └───────────────┘  └────────────────┘  └──────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Vault (Sensitive Data Isolation)           │  │
│  │              (re-export from vault)              │  │
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
| **AgentContext trait** | Abstracts via trait instead of Option<T> pattern, supports PersistentContext (full deps) and StatelessContext (no persistence) |
| **Actor model messaging** | Gateway uses three Actors (Router → Session → Outbound) communicating via mpsc channels, zero-lock design |
| **Pipeline Hook extension** | Five execution points (BeforeRequest, AfterHistory, BeforeLLM, AfterToolCall, AfterResponse) with sequential/parallel strategies |
| **Feature flag compilation** | Communication channels compiled via Cargo feature flags, enable on demand |
| **No in-memory cache** | SessionManager reads/writes SQLite directly, leverages SQLite page cache to avoid consistency issues |
| **Vault sensitive data isolation** | Sensitive data completely isolated from LLM-accessible storage, injected only at runtime, supports encrypted storage |
| **Modular Skills system** | Independent skills/ module, supports Markdown + YAML frontmatter format, progressive loading |
| **Crate separation** | Core types, providers, storage, Vault, channels split into independent crates, compatibility via re-exports |

---

## Module Dependencies

```
engine
    │
    ├── re-exports from types
    │       └── Tool trait, events (ChannelType, SessionKey, InboundMessage, etc.)
    │
    ├── re-exports from providers
    │       └── LlmProvider trait, ChatRequest, ChatResponse, etc.
    │
    ├── re-exports from storage
    │       └── SqliteStore, MemoryStore trait
    │
    ├── re-exports from vault
    │       └── VaultStore, VaultInjector, crypto types
    │
    ├── re-exports from storage
    │       └── TextEmbedder, semantic search types
    │
    ├── optional: channels (feature flags)
    │       └── Telegram, Discord, Slack, etc.
    │
    └── optional: mcp (feature flags)
            └── MCP client, manager
```

---

## Key Components

### AgentContext Trait

Core abstraction that eliminates `Option<T>` runtime checks:

```rust
#[async_trait]
pub trait AgentContext: Send + Sync {
    async fn load_session(&self, key: &SessionKey) -> Session;
    async fn save_message(&self, key: &SessionKey, role: &str, content: &str, tools: Option<Vec<String>>) -> Result<(), AgentError>;
    async fn load_summary(&self, key: &str) -> Option<String>;
    fn compress_context(&self, key: &str, evicted: &[SessionMessage]);
    async fn recall_history(&self, key: &str, query_embedding: &[f32], top_k: usize) -> Result<Vec<String>>;
    fn is_persistent(&self) -> bool;
}
```

| Implementation | Purpose |
|---------------|---------|
| `PersistentContext` | Main agent, full persistence |
| `StatelessContext` | Subagent, no persistence |

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
    Merge,            // Branch merge point
}
```

**Session Aggregate** - Aggregate root managing branch state:
```rust
pub struct Session {
    pub id: String,
    pub branches: HashMap<String, Uuid>,  // branch_name -> head_event_id
    pub current_branch: String,
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
| `storage` | SQLite storage + embedding | types, sqlx |
| `vault` | Vault encrypted storage | XChaCha20-Poly1305, Argon2 |
| `channels` | Communication channels | teloxide, serenity, etc. |
| `sandbox` | Sandbox execution | sandbox |
| `tantivy` | Full-text search MCP server | tantivy |
