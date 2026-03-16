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

## 3. Session and History Structures

### 3.1 Session

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

### 3.2 History Processing Configuration

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

## 4. Memory Structures

### 4.1 MemoryEntry

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

### 4.2 MemoryQuery

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

## 5. Vault Data Structures

### 5.1 VaultEntryV2

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

### 5.2 VaultFileV2

```rust
VaultFileV2 {
    version: String,                  // "2.0"
    entries: HashMap<String, VaultEntryV2>,
    encryption: Option<EncryptedData>,
    kdf_params: Option<KdfParams>,    // Key derivation parameters
}
```

### 5.3 InjectionReport

```rust
InjectionReport {
    total_placeholders: usize,        // Total placeholder count
    replaced: usize,                  // Successfully replaced count
    missing_keys: Vec<String>,        // Keys not found
}
```

---

## 6. SQLite Database Structure

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
├── session_summaries     Session summaries (generated by ContextCompressionHook)
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

## 7. File System Storage Structure

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
│   └── secrets.json        Encrypted key storage (AES-256-GCM)
```

> **Bootstrap file loading order**: PROFILE.md → SOUL.md → AGENTS.md → MEMORY.md → BOOTSTRAP.md
>
> MEMORY.md has a 2048 token hard limit; when exceeded, automatically truncates keeping the tail (newest content).
