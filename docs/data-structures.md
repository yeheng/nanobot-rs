# 数据结构设计

> Nanobot-RS 核心数据结构定义

---

## 1. 消息流转结构

### 1.1 入站消息 (外部 → Agent)

```rust
InboundMessage {
    channel: ChannelType,             // 枚举: Telegram | Discord | Slack | Feishu | Email |
                                      //       DingTalk | WeCom | WebSocket | Cli | Custom(String)
    sender_id: String,                // 发送者 ID
    chat_id: String,                  // 对话 ID
    content: String,                  // 消息正文
    media: Option<Vec<MediaAttachment>>,
    metadata: Option<serde_json::Value>,
    timestamp: DateTime<Utc>,
    trace_id: Option<String>,
}
```

### 1.2 出站消息 (Agent → 外部)

```rust
OutboundMessage {
    channel: ChannelType,
    chat_id: String,
    content: String,
    metadata: Option<serde_json::Value>,
    trace_id: Option<String>,
    ws_message: Option<WebSocketMessage>,  // WebSocket 实时消息
}

WebSocketMessage {
    msg_type: WebSocketMessageType,  // Text | Thinking | ToolStart | ToolEnd | TokenStats | Error | Done
    content: String,
    metadata: Option<serde_json::Value>,
}
```
```

### 1.3 会话标识

```rust
// 强类型会话标识符（替代原来的字符串拼接）
SessionKey {
    channel: ChannelType,     // 渠道类型
    chat_id: String,          // 对话 ID
}
// 序列化格式: "{channel}:{chat_id}"
// 示例: "telegram:12345", "cli:interactive"
```

### 1.4 渠道类型

```rust
enum ChannelType {
    Telegram,
    Discord,
    Slack,
    Feishu,
    Email,
    DingTalk,
    WeCom,
    WebSocket,  // WebSocket 实时通信渠道
    Cli,        // 命令行交互
    Custom(String),  // 可扩展的自定义渠道
}
```

### 1.5 媒体附件

```rust
MediaAttachment {
    media_type: String,       // MIME 类型
    url: Option<String>,      // 远程 URL
    data: Option<Vec<u8>>,    // 内联数据
    filename: Option<String>,
}
```

---

## 2. LLM 请求/响应结构

### 2.1 ChatRequest

```rust
ChatRequest {
    model: String,                        // 如 "deepseek-chat"
    messages: Vec<ChatMessage>,           // 对话历史
    tools: Option<Vec<ToolDefinition>>,   // 可用工具
    temperature: Option<f32>,             // 0.0 ~ 2.0
    max_tokens: Option<u32>,              // 最大生成 token
    thinking: Option<ThinkingConfig>,     // 推理/思考模式
}
```

### 2.2 ChatMessage

> **注意**: `role` 字段已从 `String` 改为强类型 `MessageRole` 枚举。

```rust
ChatMessage {
    role: MessageRole,                    // 强类型角色枚举
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,    // assistant 发起的工具调用
    tool_call_id: Option<String>,         // tool result 的对应 ID
    name: Option<String>,                 // tool name
}

// 角色类型 (serde 序列化为小写: "system", "user", "assistant", "tool")
enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

// 工厂方法:
ChatMessage::system(content)
ChatMessage::user(content)
ChatMessage::assistant(content)
ChatMessage::assistant_with_tools(content, tool_calls)
ChatMessage::tool_result(id, name, content)
```

### 2.3 ChatResponse

```rust
ChatResponse {
    content: Option<String>,              // 文本回复
    tool_calls: Vec<ToolCall>,            // 工具调用请求
    reasoning_content: Option<String>,    // 推理/思考内容 (DeepSeek R1 等)
    token_usage: Option<TokenUsage>,      // Token 使用量统计
}

TokenUsage {
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
}
```
```

### 2.4 ToolCall / ToolDefinition

```rust
ToolCall {
    id: String,
    r#type: String,           // "function"
    function: FunctionCall {
        name: String,
        arguments: String,    // JSON 字符串
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
    budget_tokens: Option<u32>,  // 推理预算 (token 数)
}
```

---

## 3. 会话与历史结构

### 3.1 Session

```rust
Session {
    key: String,                          // 会话标识 (如 "telegram:12345")
    messages: Vec<SessionMessage>,        // 消息列表
    last_consolidated: usize,             // 上次合并位置
}

SessionMessage {
    role: MessageRole,                    // 强类型角色
    content: String,
    timestamp: DateTime<Utc>,
    tools_used: Option<Vec<String>>,      // 使用的工具列表
}
```

### 3.2 历史处理配置

```rust
HistoryConfig {
    max_messages: usize,      // 最大消息条数 (默认 50)
    token_budget: usize,      // token 预算 (默认 4096)
    recent_keep: usize,       // 始终保留最近 N 条 (默认 4)
}

ProcessedHistory {
    messages: Vec<SessionMessage>,        // 保留的消息
    evicted: Vec<SessionMessage>,         // 被驱逐的消息 (用于摘要)
    total_tokens: usize,                  // 总 token 数
}
```

---

## 4. 记忆结构

### 4.1 MemoryEntry

```rust
MemoryEntry {
    id: String,                           // 唯一标识
    content: String,                      // 记忆内容
    metadata: MemoryMetadata,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

MemoryMetadata {
    source: Option<String>,               // 来源: "user" | "agent" | "system"
    tags: Vec<String>,                    // 分类标签
    extra: serde_json::Value,             // 可扩展键值对
}
```

### 4.2 MemoryQuery

```rust
MemoryQuery {
    text: Option<String>,                 // 全文/语义搜索
    tags: Vec<String>,                    // 按标签过滤 (AND 语义)
    source: Option<String>,              // 按来源过滤
    limit: Option<usize>,                // 结果数量限制
    offset: Option<usize>,              // 分页偏移
}
```

---

## 5. Vault 数据结构

### 5.1 VaultEntryV2

```rust
VaultEntryV2 {
    key: String,                      // 密钥名称
    value: String,                    // 密钥值 (可加密)
    description: Option<String>,      // 描述
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
    kdf_params: Option<KdfParams>,    // 密钥派生参数
}
```

### 5.3 InjectionReport

```rust
InjectionReport {
    total_placeholders: usize,        // 占位符总数
    replaced: usize,                  // 成功替换数
    missing_keys: Vec<String>,        // 未找到的密钥
}
```

---

## 6. SQLite 数据库结构

```
~/.nanobot/nanobot.db  (SqliteStore — sqlx::SqlitePool)
│
├── sessions              会话元数据
│   ├── key TEXT PK       会话标识 (如 "cli:interactive", "telegram:12345")
│   └── last_consolidated INTEGER
│
├── session_messages      每条消息独立存储 (O(1) 追加)
│   ├── id INTEGER PK
│   ├── session_key TEXT  → sessions.key
│   ├── role TEXT         "user" | "assistant" | "system" | "tool"
│   ├── content TEXT      消息内容
│   ├── timestamp TEXT    ISO 8601
│   └── tools_used TEXT   JSON 数组 (nullable)
│
├── session_summaries     会话摘要 (ContextCompressionHook 生成)
│   ├── session_key TEXT PK  → sessions.key
│   └── summary TEXT         摘要内容
│
├── memories              FTS5 全文搜索
│   ├── id TEXT PK
│   ├── content TEXT      记忆内容
│   ├── source TEXT       来源标识
│   ├── created_at TEXT
│   └── updated_at TEXT
│
├── memory_tags           记忆标签
│   ├── memory_id TEXT    → memories.id
│   └── tag TEXT
│
├── kv_store              键值对
│   ├── key TEXT PK       如 "MEMORY"
│   └── value TEXT        工作空间文件内容
│
├── cron_jobs             定时任务
│   ├── id TEXT PK
│   ├── name TEXT
│   ├── cron_expr TEXT    cron 表达式
│   ├── message TEXT      触发时发送的消息
│   ├── channel TEXT
│   ├── chat_id TEXT
│   ├── last_run TEXT
│   └── next_run TEXT
│
│  ─── 高级搜索 (已迁移到 tantivy-mcp MCP 服务) ───
│
├── (tantivy-mcp 服务)         独立的 MCP 服务器提供全文搜索
```

---

## 7. 文件系统存储结构

```
~/.nanobot/                 工作空间根目录
├── config.yaml             主配置文件
├── nanobot.db              SQLite 数据库
├── PROFILE.md              Agent 角色/人格定义
├── SOUL.md                 Agent 灵魂/价值观定义
├── AGENTS.md               Agent 行为/能力描述
├── BOOTSTRAP.md            启动引导信息
├── MEMORY.md               长期记忆 (带 token 硬截断保护)
├── hooks/                  外部 Shell Hook 脚本
│   ├── pre_request.sh      请求预处理
│   └── post_response.sh    响应后处理
├── memory/                 扩展记忆目录
├── skills/                 用户自定义技能
│   └── *.md                Markdown + YAML frontmatter
├── vault/                  敏感数据隔离目录
│   └── secrets.json        加密存储的密钥 (AES-256-GCM)
```

> **Bootstrap 文件加载顺序**: PROFILE.md → SOUL.md → AGENTS.md → MEMORY.md → BOOTSTRAP.md
>
> MEMORY.md 有 2048 token 硬限制，超出时自动截断保留尾部（最新内容）。
