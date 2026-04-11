# 数据结构设计

> Gasket-RS 核心数据结构定义

---

## 1. 消息流转结构

### 1.1 入站消息 (外部 → Agent)

> **来源**: `gasket-types::events::InboundMessage`

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

> **来源**: `gasket-types::events::OutboundMessage`

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

> **来源**: `gasket-types::events::SessionKey`

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

> **来源**: `gasket-types::events::ChannelType`

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

> **来源**: `gasket-types::events::MediaAttachment`

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

> **来源**: `gasket-providers::ChatRequest`

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

> **来源**: `gasket-providers::ChatMessage`
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

> **来源**: `gasket-providers::ChatResponse`

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

> **来源**: `gasket-providers::{ToolCall, ToolDefinition}`

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

> **来源**: `gasket-providers::ThinkingConfig`

```rust
ThinkingConfig {
    enabled: bool,
    budget_tokens: Option<u32>,  // 推理预算 (token 数)
}
```

### 2.6 流式输出类型

> **来源**: `gasket-engine::kernel::stream::StreamEvent`

```rust
pub enum StreamEvent {
    Content(String),                    // 流式内容片段
    Reasoning(String),                  // 推理/思考内容
    ToolStart { name: String, arguments: String },  // 工具调用开始
    ToolEnd { name: String, output: String },       // 工具调用结束
    TokenStats { input: u32, output: u32 },         // Token 统计
    Done,                               // 流结束
}
```

---

## 3. 事件溯源架构

### 3.1 SessionEvent

表示会话历史中单个事件的不可变事实记录。使用 UUID v7 时间有序标识符进行自然时间排序和数据库友好的索引。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 事件唯一标识符（UUID v7 时间有序）
    pub id: Uuid,

    /// 此事件所属的会话（格式: "channel:chat_id"）
    pub session_key: String,

    /// 事件类型
    pub event_type: EventType,

    /// 消息内容
    pub content: String,

    /// 语义向量（每条消息的嵌入）
    pub embedding: Option<Vec<f32>>,

    /// 事件元数据
    pub metadata: EventMetadata,

    /// 创建时间戳
    pub created_at: DateTime<Utc>,

    /// 单调递增序列号（用于水印压缩和增量查询）
    pub sequence: i64,
}
```

**关键设计点：**
- **UUID v7**：时间有序 UUID 提供自然时间排序，无需时间戳索引
- **sequence**：单调递增序列号，用于水印压缩和 `get_events_after_sequence()` 增量查询
- **嵌入**：可选的语义向量，用于相似性搜索和上下文检索
- **不可变**：事件仅追加，修改创建新事件
- **session_key**：格式为 `"channel:chat_id"`（如 `"telegram:12345"`），同时拆分存储到 `channel` 和 `chat_id` 列

### 3.2 EventType 枚举

表示系统中所有可能事件类型的判别联合。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    /// 用户消息
    UserMessage,

    /// 助手回复
    AssistantMessage,

    /// 工具调用
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// 工具结果
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
    },

    /// 摘要事件（压缩生成）
    Summary {
        summary_type: SummaryType,
        covered_event_ids: Vec<Uuid>,
    },
}
```

**事件类型类别：**
- **简单变体**：`UserMessage`、`AssistantMessage` - 基本消息类型
- **工具变体**：`ToolCall`、`ToolResult` - 工具执行生命周期
- **元变体**：`Summary` - 历史管理的系统生成事件

### 3.3 SummaryType

指定用于生成摘要事件的策略。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummaryType {
    /// 时间窗口摘要
    TimeWindow { duration_hours: u32 },

    /// 主题摘要
    Topic { topic: String },

    /// 压缩摘要（超过 token 预算时）
    Compression { token_budget: usize },
}
```

**摘要策略：**
- **TimeWindow**：汇总特定时间范围内的事件
- **Topic**：汇总与特定主题相关的事件（通过嵌入相似性提取）
- **Compression**：超过 token 预算时触发的激进摘要

### 3.4 EventMetadata

事件的可扩展元数据容器。

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// 分支名称（None 表示主分支）
    pub branch: Option<String>,

    /// 使用的工具列表
    #[serde(default)]
    pub tools_used: Vec<String>,

    /// Token 统计
    pub token_usage: Option<TokenUsage>,

    /// 内容 token 长度（写入时计算，避免读取路径重算）
    #[serde(default)]
    pub content_token_len: usize,

    /// 扩展字段
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
```

**字段：**
- **branch**：类似 Git 的分支支持；`None` 表示主分支
- **tools_used**：跟踪在此事件的处理期间调用了哪些工具
- **token_usage**：LLM token 消耗统计，用于成本跟踪
- **content_token_len**：写入时一次性计算的 content token 数，避免读取路径重算
- **extra**：开放式的键值存储，用于未来扩展而无需架构更改

### 3.5 Session（聚合根）

管理会话状态和分支指针的聚合根。

```rust
#[derive(Debug, Clone)]
pub struct Session {
    /// 会话标识符
    pub key: String,

    /// 当前活动分支
    pub current_branch: String,

    /// 所有分支指针（branch_name -> latest_event_id）
    pub branches: HashMap<String, Uuid>,

    /// 会话元数据
    pub metadata: SessionMetadata,
}
```

**职责：**
- 维护新事件的当前分支上下文
- 跟踪每个分支的头提交
- 提供会话级别的元数据和统计信息

### 3.6 SessionMetadata

会话级别的统计和维护信息。

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// 创建时间戳
    pub created_at: DateTime<Utc>,

    /// 最后更新时间戳
    pub updated_at: DateTime<Utc>,

    /// 最后合并点（事件 ID）
    pub last_consolidated_event: Option<Uuid>,

    /// 总消息数
    pub total_events: usize,

    /// 累计 token 使用量
    pub total_tokens: u64,
}
```

**用途：**
- **last_consolidated_event**：跟踪包含在摘要中的最后一个事件；用于增量摘要
- **total_events/total_tokens**：资源监控和限制的运行计数器

---

## 4. 会话与历史结构（旧版）

### 3.1 Session

> **来源**: `gasket-types::session_event::Session`

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

> **来源**: `gasket_storage::history`

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

### 4.8 AgentContext (基于枚举)

零成本枚举分发 — 无运行时开销。

```rust
#[derive(Debug, Clone)]
pub enum AgentContext {
    /// 持久化上下文（主 Agent）
    Persistent(PersistentContext),

    /// 无状态上下文（子 Agent）
    Stateless,
}

/// 主代理的持久化上下文数据。
#[derive(Clone)]
pub struct PersistentContext {
    /// 用于持久化事件的事件存储
    pub event_store: Arc<EventStore>,
    /// 用于保存嵌入的 SQLite 存储（语义召回索引）
    pub sqlite_store: Arc<SqliteStore>,
    /// 可选的文本嵌入器，用于自动生成嵌入
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}
```

**AgentContext 上的关键方法：**

| 方法 | 描述 |
|------|------|
| `persistent(event_store, sqlite_store) -> Self` | 创建持久化变体 |
| `is_persistent(&self) -> bool` | 检查变体 |
| `load_session(&self, key) -> Session` | 从事件存储加载 |
| `save_event(&self, event) -> Result` | 追加事件 |
| `get_history(&self, key, branch) -> Vec<SessionEvent>` | 获取分支历史 |
| `recall_history(&self, key, embedding, top_k) -> Vec<String>` | 语义召回 |
| `clear_session(&self, key) -> Result` | 清除会话 |

**变体：**

| 变体 | 用途 |
|------|------|
| `Persistent(PersistentContext)` | 主代理，完整事件溯源 |
| `Stateless` | 子代理，无持久化 |

**设计优势：**
- 零运行时分发开销（枚举分发 vs 特征对象 vtable）
- 更好的缓存局部性（枚举变体内联）
- 编译时穷举检查

### 4.9 ContextCompactor

上下文压缩器 — 在 token 预算超限时触发摘要生成。

```rust
pub struct ContextCompactor {
    provider: Arc<dyn LlmProvider>,
    event_store: Arc<EventStore>,
    sqlite_store: Arc<SqliteStore>,
    model: String,
    token_budget: usize,
}

impl ContextCompactor {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        event_store: Arc<EventStore>,
        sqlite_store: Arc<SqliteStore>,
        model: String,
        token_budget: usize,
    ) -> Self;

    pub fn with_summarization_prompt(self, prompt: impl Into<String>) -> Self;

    /// 非阻塞压缩检查
    pub fn try_compact(
        &self,
        session_key: &str,
        estimated_tokens: usize,
        vault_values: &[String],
    );
}
```

**关键设计点：**
- **非阻塞执行**：`try_compact` 生成异步任务，不阻塞响应
- **Token 预算检查**：当 `estimated_tokens` 超过预算时触发压缩
- **后台摘要**：压缩在后台执行，完成后保存到 EventStore

**生命周期：**
```text
AgentSession::process_direct()
  → prepare_pipeline()     // 历史 + 提示组装
  → kernel::execute()      // LLM 迭代 (纯函数)
  → finalize_response()    // 保存事件 + 压缩 + 返回
```

---

## 4. 记忆结构

### 4.1 MemoryMeta

> **来源**: `gasket-storage::memory::types`

记忆文件的 YAML 前置元数据。

```rust
MemoryMeta {
    id: String,                           // UUID v4 唯一标识
    title: String,                        // 人类可读标题
    r#type: String,                       // 记忆类型（自由标签）
    scenario: Scenario,                   // 场景分类（6 种）
    tags: Vec<String>,                    // 用户定义标签
    frequency: Frequency,                 // 访问频率（hot/warm/cold/archived）
    access_count: u64,                    // 访问次数
    created: String,                      // RFC 3339 时间戳
    updated: String,                      // 最后更新时间
    last_accessed: String,                // 最后访问时间
    auto_expire: bool,                    // 是否自动过期
    expires: Option<String>,              // 过期时间
    tokens: usize,                        // token 数量估算
    superseded_by: Option<String>,        // 替代此记忆的新记忆 ID
}
```

### 4.2 MemoryFile

> **来源**: `gasket-storage::memory::types`

完整的记忆文件（元数据 + Markdown 内容）。

```rust
MemoryFile {
    metadata: MemoryMeta,                 // YAML 前置元数据
    content: String,                      // Markdown 正文
}
```

### 4.3 MemoryQuery

> **来源**: `gasket-storage::memory::types`

记忆搜索查询参数。

```rust
MemoryQuery {
    text: Option<String>,                 // 全文/语义搜索
    tags: Vec<String>,                    // 按标签过滤（ANY 语义）
    scenario: Option<Scenario>,           // 场景过滤
    max_tokens: Option<usize>,            // 最大 token 限制
}
```

### 4.4 MemoryHit

> **来源**: `gasket-storage::memory::types`

带相关性评分的搜索结果。

```rust
MemoryHit {
    path: String,                         // 相对于 ~/.gasket/memory/ 的路径
    scenario: Scenario,                   // 场景分类
    title: String,                        // 标题
    tags: Vec<String>,                    // 标签
    frequency: Frequency,                 // 频率
    score: f32,                           // 相关性评分 (0.0–1.0)
    tokens: usize,                        // token 数
}
```

### 4.5 MemoryProvider Trait

> **来源**: `gasket-engine::session::store::MemoryProvider`

解耦 HistoryCoordinator 与 MemoryManager 的窄接口。

```rust
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext>;
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>>;
    async fn update_from_event(&self, event: &SessionEvent) -> Result<()>;
    async fn create_memory(&self, scenario, filename, title, tags, frequency, content) -> Result<()>;
    async fn update_memory(&self, scenario, filename, content) -> Result<()>;
    async fn delete_memory(&self, scenario, filename) -> Result<()>;
}
```

### 4.6 MemoryContext

> **来源**: `gasket-engine::session::memory::MemoryManager`

三阶段加载的结果。

```rust
MemoryContext {
    memories: Vec<MemoryFile>,            // 加载的记忆文件（在 token 预算内）
    tokens_used: usize,                   // 实际使用的 token 数
    phase_breakdown: PhaseBreakdown,      // 分阶段 token 明细
}

PhaseBreakdown {
    bootstrap_tokens: usize,              // 阶段 1 (profile + active)
    scenario_tokens: usize,               // 阶段 2 (场景 hot/warm)
    on_demand_tokens: usize,              // 阶段 3 (搜索填充)
}
```

---

## 5. Vault 数据结构

### 5.1 VaultEntryV2

> **来源**: `gasket-engine::vault::VaultEntryV2`

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

> **来源**: `gasket-engine::vault::VaultFileV2`

```rust
VaultFileV2 {
    version: String,                  // "2.0"
    entries: HashMap<String, VaultEntryV2>,
    encryption: Option<EncryptedData>,
    kdf_params: Option<KdfParams>,    // 密钥派生参数
}
```

### 5.3 InjectionReport

> **来源**: `gasket-engine::vault::injector`

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
~/.gasket/gasket.db  (SqliteStore — sqlx::SqlitePool)
│
│  ─── 会话管理（V2 事件溯源） ───
│
├── sessions_v2              会话元数据（V2）
│   ├── key TEXT PK          会话标识 (如 "cli:interactive", "telegram:12345")
│   ├── channel TEXT         渠道类型 (如 "telegram", "cli")
│   ├── chat_id TEXT         对话 ID
│   ├── current_branch TEXT  当前分支 (默认 "main")
│   ├── branches TEXT        分支指针 JSON (branch_name → event_id)
│   ├── created_at TEXT
│   ├── updated_at TEXT
│   ├── last_consolidated_event TEXT
│   ├── total_events INTEGER 事件计数
│   └── total_tokens INTEGER Token 累计
│
├── session_events           事件日志（仅追加，不可变）
│   ├── id TEXT PK           UUID v7 时间有序
│   ├── session_key TEXT     → sessions_v2.key
│   ├── channel TEXT         冗余存储，加速渠道查询
│   ├── chat_id TEXT         冗余存储，加速对话查询
│   ├── event_type TEXT      "user_message" | "assistant_message" | "tool_call" | "tool_result" | "summary"
│   ├── content TEXT         消息内容
│   ├── embedding BLOB       可选 f32 向量
│   ├── branch TEXT          分支名 (默认 "main")
│   ├── tools_used TEXT      JSON 数组
│   ├── token_usage TEXT     JSON TokenUsage
│   ├── token_len INTEGER    内容 token 数（写入时计算）
│   ├── event_data TEXT      工具/摘要类型详情 JSON
│   ├── extra TEXT           扩展 JSON
│   ├── created_at TEXT      ISO 8601
│   └── sequence INTEGER     单调递增序列号（水印压缩用）
│   索引: idx_events_session_sequence ON (session_key, sequence)
│
├── session_summaries        会话摘要检查点
│   ├── session_key TEXT PK
│   ├── content TEXT         摘要内容
│   ├── covered_upto_sequence INTEGER  水印：覆盖到此序列号
│   └── created_at TEXT
│
├── summary_index            摘要事件索引
│   ├── id INTEGER PK AUTOINCREMENT
│   ├── session_key TEXT
│   ├── event_id TEXT        摘要事件的 UUID
│   ├── summary_type TEXT    摘要类型标签
│   ├── topic TEXT           主题摘要的主题
│   ├── covered_events TEXT  覆盖的事件 ID JSON 数组
│   └── created_at TEXT
│
├── session_embeddings       事件嵌入索引
│   ├── message_id TEXT PK
│   ├── session_key TEXT     → sessions_v2.key
│   ├── embedding BLOB       f32 向量
│   └── created_at TEXT
│
│  ─── 记忆系统 ───
│
├── memory_metadata          记忆文件元数据（替代旧 _INDEX.md）
│   ├── id TEXT              记忆 ID (mem_xxx)
│   ├── path TEXT            文件名 (mem_xxx.md)
│   ├── scenario TEXT        场景目录名
│   ├── title TEXT           标题
│   ├── memory_type TEXT     类型 (默认 "note")
│   ├── frequency TEXT       "hot" | "warm" | "cold" | "archived"
│   ├── tags TEXT            JSON 数组 (json_each 查询)
│   ├── tokens INTEGER       token 数
│   ├── updated TEXT         更新时间
│   ├── last_accessed TEXT   最后访问时间
│   ├── file_mtime BIGINT    文件修改时间（纳秒）
│   └── PRIMARY KEY (scenario, path)
│
├── memory_embeddings        记忆嵌入向量
│   ├── memory_path TEXT PK  文件路径
│   ├── scenario TEXT        场景
│   ├── tags TEXT            JSON 数组
│   ├── frequency TEXT       频率
│   ├── embedding BLOB       f32 向量
│   ├── token_count INTEGER  token 数
│   ├── created_at TEXT
│   └── updated_at TEXT
│
│  ─── 通用存储 ───
│
├── kv_store                 键值对
│   ├── key TEXT PK          如 "MEMORY"
│   ├── value TEXT           工作空间文件内容
│   └── updated_at TEXT
│
├── cron_jobs                定时任务
│   ├── id TEXT PK
│   ├── name TEXT
│   ├── cron TEXT            cron 表达式
│   ├── message TEXT         触发时发送的消息
│   ├── channel TEXT
│   ├── chat_id TEXT
│   ├── last_run TEXT
│   ├── next_run TEXT
│   └── enabled INTEGER     是否启用
│
│  ─── 高级搜索 (已迁移到 tantivy-mcp MCP 服务) ───
│
├── (tantivy-mcp 服务)        独立的 MCP 服务器提供全文搜索
```

### 水印压缩设计

事件存储采用 **高水位标记 (High-Water Mark)** 压缩策略：

```
写入路径:
  append_event() → 自动生成 sequence (MAX + 1)
                  → 写入 session_events
                  → 更新 sessions_v2.branches JSON

读取路径（压缩恢复）:
  1. get_latest_summary() → 获取最新摘要事件
  2. summary.covered_upto_sequence → 水印值
  3. get_events_after_sequence(watermark) → 仅加载水印后的事件
  4. 重组上下文 = 摘要内容 + 增量事件

压缩路径:
  1. get_events_up_to_sequence(target) → 获取待压缩事件
  2. LLM 生成摘要 → 写入新的 Summary 事件
  3. delete_events_upto(target) → 清理已压缩的旧事件
```

---

## 7. 文件系统存储结构

```
~/.gasket/                 工作空间根目录
├── config.yaml             主配置文件
├── gasket.db              SQLite 数据库
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
│   └── secrets.json        加密存储的密钥 (XChaCha20-Poly1305)
```

> **Bootstrap 文件加载顺序**: PROFILE.md → SOUL.md → AGENTS.md → MEMORY.md → BOOTSTRAP.md
>
> MEMORY.md 有 2048 token 硬限制，超出时自动截断保留尾部（最新内容）。

---

## 8. 子代理 (Subagent) 结构

### 8.1 SubagentManager

> **来源**: `gasket-engine::subagents::manager`

```rust
pub struct SubagentManager {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    session_key: Arc<RwLock<Option<SessionKey>>>,
    timeout_secs: u64,
}
```

### 8.2 SubagentTaskBuilder

Builder 模式用于配置子代理任务：

```rust
pub struct SubagentTaskBuilder<'a> {
    manager: &'a SubagentManager,
    subagent_id: String,
    task: String,
    provider: Option<Arc<dyn LlmProvider>>,
    config: Option<AgentConfig>,
    event_tx: Option<mpsc::Sender<SubagentEvent>>,
    system_prompt: Option<String>,
    session_key: Option<SessionKey>,
    cancellation_token: Option<CancellationToken>,
    hooks: Option<Arc<HookRegistry>>,
}
```

### 8.3 SubagentEvent

> **来源**: `gasket-engine::subagents::tracker`

```rust
pub enum SubagentEvent {
    Thinking { subagent_id: String, content: String },
    ToolStart { subagent_id: String, name: String, arguments: String },
    ToolEnd { subagent_id: String, name: String, output: String },
    Content { subagent_id: String, content: String },
    Completed { subagent_id: String, result: SubagentResult },
}

pub struct SubagentResult {
    pub subagent_id: String,
    pub content: String,
    pub success: bool,
}
```

---

## 9. Hook 系统结构

### 9.1 Hook 类型定义

> **来源**: `gasket-engine::hooks::types`

```rust
pub enum HookPoint {
    BeforeRequest,    // 请求处理前
    AfterHistory,     // 历史加载后
    BeforeLLM,        // 发送给 LLM 前
    AfterToolCall,    // 工具调用后
    AfterResponse,    // 响应生成后
}

pub enum ExecutionStrategy {
    Sequential,       // 顺序执行，可修改状态
    Parallel,         // 并行执行，只读
}

pub enum HookAction {
    Continue,         // 继续执行
    Abort { reason: String },  // 中止请求
}
```

### 9.2 HookContext

```rust
pub struct HookContext {
    pub session_key: SessionKey,
    pub messages: Vec<ChatMessage>,
    pub metadata: HashMap<String, String>,
}

pub struct MutableContext<'a> {
    pub messages: &'a mut Vec<ChatMessage>,
    pub metadata: &'a mut HashMap<String, String>,
}

pub struct ReadonlyContext<'a> {
    pub messages: &'a [ChatMessage],
    pub metadata: &'a HashMap<String, String>,
}
```
