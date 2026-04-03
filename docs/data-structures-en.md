# Data Structure Design

> Gasket-RS Core Data Structure Definitions

---

## 1. Message Flow Structures

### 1.1 Inbound Message (External → Agent)

```rust
InboundMessage {
    channel: ChannelType,             // Enum: Telegram | Discord | Slack | Feishu | Email |
                                      //       DingTalk | WeCom | WebSocket | Cli | Custom(String)
    sender_id: String,                // Sender ID
    chat_id: String,                  // Chat ID
    content: String,                  // Message content
    media: Option<Vec<MediaAttachment>>,
    metadata: Option<serde_json::Value>,
    timestamp: DateTime<Utc>,
    trace_id: Option<String>,
}
```

### 1.2 Outbound Message (Agent → External)

```rust
OutboundMessage {
    channel: ChannelType,
    chat_id: String,
    content: String,
    metadata: Option<serde_json::Value>,
    trace_id: Option<String>,
    ws_message: Option<WebSocketMessage>,  // WebSocket real-time message
}

WebSocketMessage {
    msg_type: WebSocketMessageType,  // Text | Thinking | ToolStart | ToolEnd | TokenStats | Error | Done
    content: String,
    metadata: Option<serde_json::Value>,
}
```

### 1.3 Session Identifier

```rust
// Strongly-typed session identifier (replaces string concatenation)
SessionKey {
    channel: ChannelType,     // Channel type
    chat_id: String,          // Chat ID
}
// Serialization format: "{channel}:{chat_id}"
// Examples: "telegram:12345", "cli:interactive"
```

### 1.4 Channel Types

```rust
enum ChannelType {
    Telegram,
    Discord,
    Slack,
    Feishu,
    Email,
    DingTalk,
    WeCom,
    WebSocket,  // WebSocket real-time communication channel
    Cli,        // Command line interaction
    Custom(String),  // Extensible custom channels
}
```

### 1.5 Media Attachments

```rust
MediaAttachment {
    media_type: String,       // MIME type
    url: Option<String>,      // Remote URL
    data: Option<Vec<u8>>,    // Inline data
    filename: Option<String>,
}
```

---

## 2. LLM Request/Response Structures

### 2.1 ChatRequest

```rust
ChatRequest {
    model: String,                        // e.g., "deepseek-chat"
    messages: Vec<ChatMessage>,           // Conversation history
    tools: Option<Vec<ToolDefinition>>,   // Available tools
    temperature: Option<f32>,             // 0.0 ~ 2.0
    max_tokens: Option<u32>,              // Max generation tokens
    thinking: Option<ThinkingConfig>,     // Reasoning/thinking mode
}
```

### 2.2 ChatMessage

> **Note**: The `role` field has been changed from `String` to strongly-typed `MessageRole` enum.

```rust
ChatMessage {
    role: MessageRole,                    // Strongly-typed role enum
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,    // Tool calls initiated by assistant
    tool_call_id: Option<String>,         // Corresponding ID for tool result
    name: Option<String>,                 // Tool name
}

// Role types (serde serializes to lowercase: "system", "user", "assistant", "tool")
enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

// Factory methods:
ChatMessage::system(content)
ChatMessage::user(content)
ChatMessage::assistant(content)
ChatMessage::assistant_with_tools(content, tool_calls)
ChatMessage::tool_result(id, name, content)
```

### 2.3 ChatResponse

```rust
ChatResponse {
    content: Option<String>,              // Text response
    tool_calls: Vec<ToolCall>,            // Tool call requests
    reasoning_content: Option<String>,    // Reasoning/thinking content (DeepSeek R1, etc.)
    token_usage: Option<TokenUsage>,      // Token usage statistics
}

TokenUsage {
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
}
```

### 2.4 ToolCall / ToolDefinition

```rust
ToolCall {
    id: String,
    r#type: String,           // "function"
    function: FunctionCall {
        name: String,
        arguments: String,    // JSON string
    },
}

ToolDefinition {
    r#type: String,           // "function"
    function: FunctionDefinition {
        name: String,
        description: String,
        parameters: serde_json::Value,  // JSON Schema
    },
}
```

### 2.5 ThinkingConfig

```rust
ThinkingConfig {
    enabled: bool,
    budget_tokens: Option<u32>,  // Reasoning budget (token count)
}
```

---

## 3. Event Sourcing Architecture

### 3.1 SessionEvent

Immutable fact record representing a single event in the session history. Uses UUID v7 time-ordered identifiers for natural chronological sorting and database-friendly indexing.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Event unique identifier (UUID v7 time-ordered)
    pub id: Uuid,

    /// Session this event belongs to
    pub session_key: String,

    /// Event type
    pub event_type: EventType,

    /// Message content
    pub content: String,

    /// Semantic vector (per-message embedding)
    pub embedding: Option<Vec<f32>>,

    /// Event metadata
    pub metadata: EventMetadata,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}
```

**Key Design Points:**
- **UUID v7**: Time-ordered UUIDs provide natural chronological sorting without requiring timestamp indexes
- **Embedding**: Optional semantic vector for similarity search and context retrieval
- **Immutable**: Events are append-only; modifications create new events

### 3.2 EventType Enum

Discriminated union representing all possible event types in the system.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    /// User message
    UserMessage,

    /// Assistant reply
    AssistantMessage,

    /// Tool call
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// Tool result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
    },

    /// Summary event (compression generated)
    Summary {
        summary_type: SummaryType,
        covered_event_ids: Vec<Uuid>,
    },
}
```

**Event Type Categories:**
- **Simple variants**: `UserMessage`, `AssistantMessage` - basic message types
- **Tool variants**: `ToolCall`, `ToolResult` - tool execution lifecycle
- **Meta variants**: `Summary` - system-generated events for history management

### 3.3 SummaryType

Specifies the strategy used to generate a summary event.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummaryType {
    /// Time window summary
    TimeWindow { duration_hours: u32 },

    /// Topic summary
    Topic { topic: String },

    /// Compression summary (when exceeding token budget)
    Compression { token_budget: usize },
}
```

**Summary Strategies:**
- **TimeWindow**: Summarize events within a specific time range
- **Topic**: Summarize events related to a specific topic (extracted via embedding similarity)
- **Compression**: Aggressive summarization triggered when token budget is exceeded

### 3.5 EventMetadata

Extensible metadata container for events.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Branch name (None means main branch)
    pub branch: Option<String>,

    /// List of tools used
    #[serde(default)]
    pub tools_used: Vec<String>,

    /// Token statistics
    pub token_usage: Option<TokenUsage>,

    /// Extension fields
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
```

**Fields:**
- **branch**: Git-like branching support; `None` indicates the main branch
- **tools_used**: Tracks which tools were invoked during this event's processing
- **token_usage**: LLM token consumption statistics for cost tracking
- **extra**: Open-ended key-value store for future extensions without schema changes

### 3.6 Session (Aggregate Root)

Aggregate root managing session state and branch pointers.

```rust
#[derive(Debug, Clone)]
pub struct Session {
    /// Session identifier
    pub key: String,

    /// Current active branch
    pub current_branch: String,

    /// All branch pointers (branch_name -> latest_event_id)
    pub branches: HashMap<String, Uuid>,

    /// Session metadata
    pub metadata: SessionMetadata,
}
```

**Responsibilities:**
- Maintains the current branch context for new events
- Tracks head commit for each branch
- Provides session-level metadata and statistics

### 3.7 SessionMetadata

Session-level statistics and housekeeping information.

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,

    /// Last consolidation point (event ID)
    pub last_consolidated_event: Option<Uuid>,

    /// Total message count
    pub total_events: usize,

    /// Cumulative token usage
    pub total_tokens: u64,
}
```

**Usage:**
- **last_consolidated_event**: Tracks the last event included in a summary; used for incremental summarization
- **total_events/total_tokens**: Running counters for resource monitoring and limits

### 3.8 AgentContext (Enum-based)

Zero-cost enum dispatch for agent state management — no runtime overhead.

```rust
#[derive(Debug, Clone)]
pub enum AgentContext {
    /// Persistent context (main Agent)
    Persistent(PersistentContext),

    /// Stateless context (sub Agent)
    Stateless,
}

/// Persistent context data for main agents.
#[derive(Clone)]
pub struct PersistentContext {
    /// Event store for persisting events
    pub event_store: Arc<EventStore>,
    /// SQLite store for saving embeddings (semantic recall index)
    pub sqlite_store: Arc<SqliteStore>,
    /// Optional text embedder for automatic embedding generation
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}
```

**Key Methods on AgentContext:**

| Method | Description |
|--------|-------------|
| `persistent(event_store, sqlite_store) -> Self` | Create persistent variant |
| `is_persistent(&self) -> bool` | Check variant |
| `load_session(&self, key) -> Session` | Load from event store |
| `save_event(&self, event) -> Result` | Append event |
| `get_history(&self, key, branch) -> Vec<SessionEvent>` | Get branch history |
| `recall_history(&self, key, embedding, top_k) -> Vec<String>` | Semantic recall |
| `clear_session(&self, key) -> Result` | Clear session |

**Variants:**

| Variant | Purpose |
|---------|---------|
| `Persistent(PersistentContext)` | Main agent, full event sourcing |
| `Stateless` | Subagent, no persistence |

**Design Benefits:**
- Zero runtime dispatch overhead (enum dispatch vs trait object vtable)
- Better cache locality (enum variants are inline)
- Compile-time exhaustiveness checking

### 3.9 ContextCompactor

Synchronous context compactor — replaces async background summarization. Called directly after each agent response to ensure the next request sees the latest summary.

```rust
pub struct ContextCompactor {
    /// LLM provider for generating summaries
    provider: Arc<dyn LlmProvider>,
    /// Event store for persisting summary events
    event_store: Arc<EventStore>,
    /// Model to use for summarization
    model: String,
    /// Token budget for context window
    token_budget: usize,
    /// Compaction threshold multiplier (default 1.2)
    compaction_threshold: f32,
    /// Custom summarization prompt
    summarization_prompt: String,
}

impl ContextCompactor {
    /// Create a new compactor
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        event_store: Arc<EventStore>,
        model: String,
        token_budget: usize,
    ) -> Self;

    /// Set a custom summarization prompt
    pub fn with_summarization_prompt(self, prompt: impl Into<String>) -> Self;

    /// Set a custom compaction threshold multiplier
    pub fn with_threshold(self, threshold: f32) -> Self;

    /// Run compaction on evicted events
    pub async fn compact(
        &self,
        session_key: &str,
        evicted_events: &[SessionEvent],
        vault_values: &[String],
    ) -> anyhow::Result<Option<String>>;
}
```

**Key Design Points:**
- **Synchronous execution**: Runs in `finalize_response()` after user receives response (no added latency)
- **No race conditions**: Next request always sees latest summary (eliminates `tokio::spawn` timing issues)
- **Batch threshold**: Only compacts when evicted tokens exceed `token_budget * (threshold - 1.0)`
- **LSM-tree analogy**: L0 (active context) flushes to L1 (summary checkpoint) on overflow

**Lifecycle:**
```text
AgentLoop::process_direct()
  → prepare_pipeline()     // history + prompt assembly
  → run_agent_loop()       // LLM iteration
  → finalize_response()    // save event + compact + return
```

---

## 4. Session and History Structures

### 4.1 Session (Legacy)

```rust
Session {
    key: String,                          // Session identifier (e.g., "telegram:12345")
    messages: Vec<SessionMessage>,        // Message list
    last_consolidated: usize,             // Last consolidation position
}

SessionMessage {
    role: MessageRole,                    // Strongly-typed role
    content: String,
    timestamp: DateTime<Utc>,
    tools_used: Option<Vec<String>>,      // List of tools used
}
```

### 4.2 History Processing Configuration

```rust
HistoryConfig {
    max_messages: usize,      // Max message count (default 50)
    token_budget: usize,      // Token budget (default 4096)
    recent_keep: usize,       // Always keep last N messages (default 4)
}

ProcessedHistory {
    messages: Vec<SessionMessage>,        // Kept messages
    evicted: Vec<SessionMessage>,         // Evicted messages (for summary)
    total_tokens: usize,                  // Total token count
}
```

---

## 5. Memory Structures

### 5.1 MemoryEntry

```rust
MemoryEntry {
    id: String,                           // Unique identifier
    content: String,                      // Memory content
    metadata: MemoryMetadata,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

MemoryMetadata {
    source: Option<String>,               // Source: "user" | "agent" | "system"
    tags: Vec<String>,                    // Classification tags
    extra: serde_json::Value,             // Extensible key-value pairs
}
```

### 5.2 MemoryQuery

```rust
MemoryQuery {
    text: Option<String>,                 // Full-text/semantic search
    tags: Vec<String>,                    // Filter by tags (AND semantics)
    source: Option<String>,              // Filter by source
    limit: Option<usize>,                // Result count limit
    offset: Option<usize>,              // Pagination offset
}
```

---

## 6. Vault Data Structures

### 6.1 VaultEntryV2

```rust
VaultEntryV2 {
    key: String,                      // Key name
    value: String,                    // Key value (can be encrypted)
    description: Option<String>,      // Description
    metadata: VaultMetadata,
}

VaultMetadata {
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_used: Option<DateTime<Utc>>,
}
```

### 6.2 VaultFileV2

```rust
VaultFileV2 {
    version: String,                  // "2.0"
    entries: HashMap<String, VaultEntryV2>,
    encryption: Option<EncryptedData>,
    kdf_params: Option<KdfParams>,    // Key derivation parameters
}
```

### 6.3 InjectionReport

```rust
InjectionReport {
    total_placeholders: usize,        // Total placeholder count
    replaced: usize,                  // Successfully replaced count
    missing_keys: Vec<String>,        // Keys not found
}
```

---

## 7. SQLite Database Structure

```
~/.gasket/gasket.db  (SqliteStore — sqlx::SqlitePool)
│
├── sessions              Session metadata
│   ├── key TEXT PK       Session identifier (e.g., "cli:interactive", "telegram:12345")
│   └── last_consolidated INTEGER
│
├── session_messages      Each message stored independently (O(1) append)
│   ├── id INTEGER PK
│   ├── session_key TEXT  → sessions.key
│   ├── role TEXT         "user" | "assistant" | "system" | "tool"
│   ├── content TEXT      Message content
│   ├── timestamp TEXT    ISO 8601
│   └── tools_used TEXT   JSON array (nullable)
│
├── session_summaries     Session summaries (generated by ContextCompactor)
│   ├── session_key TEXT PK  → sessions.key
│   └── summary TEXT         Summary content
│
├── memories              FTS5 full-text search
│   ├── id TEXT PK
│   ├── content TEXT      Memory content
│   ├── source TEXT       Source identifier
│   ├── created_at TEXT
│   └── updated_at TEXT
│
├── memory_tags           Memory tags
│   ├── memory_id TEXT    → memories.id
│   └── tag TEXT
│
├── kv_store              Key-value pairs
│   ├── key TEXT PK       e.g., "MEMORY"
│   └── value TEXT        Workspace file content
│
├── cron_jobs             Scheduled tasks
│   ├── id TEXT PK
│   ├── name TEXT
│   ├── cron_expr TEXT    Cron expression
│   ├── message TEXT      Message sent when triggered
│   ├── channel TEXT
│   ├── chat_id TEXT
│   ├── last_run TEXT
│   └── next_run TEXT
│
│  ─── Advanced Search (migrated to tantivy-mcp MCP service) ───
│
├── (tantivy-mcp service)      Standalone MCP server provides full-text search
```

---

## 8. File System Storage Structure

```
~/.gasket/                 Workspace root directory
├── config.yaml             Main configuration file
├── gasket.db              SQLite database
├── PROFILE.md              Agent role/personality definition
├── SOUL.md                 Agent soul/values definition
├── AGENTS.md               Agent behavior/capability description
├── BOOTSTRAP.md            Bootstrap information
├── MEMORY.md               Long-term memory (with token hard truncation protection)
├── hooks/                  External Shell Hook scripts
│   ├── pre_request.sh      Request preprocessing
│   └── post_response.sh    Post-response processing
├── memory/                 Extended memory directory
├── skills/                 User-defined skills
│   └── *.md                Markdown + YAML frontmatter
├── vault/                  Sensitive data isolation directory
│   └── secrets.json        Encrypted key storage (XChaCha20-Poly1305)
```

> **Bootstrap file loading order**: PROFILE.md → SOUL.md → AGENTS.md → MEMORY.md → BOOTSTRAP.md
>
> MEMORY.md has a 2048 token hard limit; when exceeded, automatically truncates keeping the tail (newest content).
