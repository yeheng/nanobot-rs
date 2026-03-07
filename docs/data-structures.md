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
}
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
}
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

## 5. Pipeline 数据结构 (opt-in)

> 以下结构仅在 `pipeline.enabled: true` 时激活。

### 5.1 TaskState (状态机)

```rust
enum TaskState {
    Pending,     // 新创建
    Triage,      // 太子分诊中
    Planning,    // 中书省规划中
    Reviewing,   // 门下省审核中
    Assigned,    // 尚书省分派中
    Executing,   // 六部执行中
    Review,      // 执行后审核
    Done,        // 已完成
    Blocked,     // 被阻塞
}
```

合法转换: `Pending→Triage→Planning→Reviewing→Assigned→Executing→Review→Done`，另有 `Reviewing→Planning`（拒绝）、`Executing/Review→Blocked`、`Blocked→Executing/Planning`（恢复）。

### 5.2 PipelineTask

```rust
PipelineTask {
    id: String,                       // UUID v4
    title: String,                    // 任务标题
    description: String,              // 详细描述
    state: TaskState,                 // 当前状态
    priority: TaskPriority,           // Low | Normal | High | Critical
    assigned_role: Option<String>,    // 当前负责角色
    review_count: u32,                // 审核轮次
    retry_count: u32,                 // 重试次数 (停滞恢复)
    last_heartbeat: DateTime<Utc>,    // 最后心跳 (停滞检测用)
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    result: Option<String>,           // 完成时的结果
    origin_channel: Option<String>,   // 来源渠道
    origin_chat_id: Option<String>,   // 来源会话
}
```

### 5.3 FlowLogEntry (审计日志)

```rust
FlowLogEntry {
    id: i64,
    task_id: String,
    from_state: String,
    to_state: String,
    agent_role: String,               // 执行转换的角色
    reason: Option<String>,           // 转换原因
    timestamp: DateTime<Utc>,
}
```

### 5.4 ProgressEntry (进度记录)

```rust
ProgressEntry {
    id: i64,
    task_id: String,
    agent_role: String,
    content: String,                  // 进度描述
    percentage: Option<f32>,          // 0.0 ~ 100.0
    timestamp: DateTime<Utc>,
}
```

### 5.5 PipelineEvent (编排器事件)

```rust
enum PipelineEvent {
    TaskCreated { task_id: String },
    TaskTransitioned { task_id: String, new_state: TaskState, agent_role: String },
    ProgressReported { task_id: String, agent_role: String },
    StallDetected { task_id: String },
}
```

### 5.6 PipelineConfig

```rust
PipelineConfig {
    enabled: bool,                    // 主开关 (默认 false)
    use_default_template: bool,       // 加载内置三省六部模板 (默认 true)
    roles: HashMap<String, AgentRoleDef>,
    max_reviews: u32,                 // 审核上限 (默认 3)
    stall_timeout_secs: u64,          // 心跳超时 (默认 60s)
    model: Option<String>,            // 全局模型覆盖
}

AgentRoleDef {
    description: String,
    allowed_agents: Vec<String>,
    soul_path: Option<String>,        // 自定义 SOUL.md 路径
    model: Option<String>,            // 角色专用模型
    responsible_states: Vec<String>,
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
│  ─── Pipeline 表 (opt-in, 仅 pipeline.enabled 时创建) ───
│
├── pipeline_tasks        管线任务看板
│   ├── id TEXT PK        UUID v4
│   ├── title TEXT        任务标题
│   ├── description TEXT  任务详情
│   ├── state TEXT        状态 (pending/triage/planning/reviewing/assigned/executing/review/done/blocked)
│   ├── priority TEXT     优先级 (low/normal/high/critical)
│   ├── assigned_role TEXT 当前负责角色
│   ├── review_count INT  审核轮次计数
│   ├── retry_count INT   重试计数 (停滞恢复)
│   ├── last_heartbeat TEXT RFC 3339 (停滞检测用)
│   ├── created_at TEXT
│   ├── updated_at TEXT
│   ├── result TEXT       完成时的结果内容 (nullable)
│   ├── origin_channel TEXT 来源渠道
│   └── origin_chat_id TEXT 来源会话
│
├── pipeline_flow_log     流转审计日志 (append-only)
│   ├── id INTEGER PK
│   ├── task_id TEXT      → pipeline_tasks.id
│   ├── from_state TEXT   原状态
│   ├── to_state TEXT     目标状态
│   ├── agent_role TEXT   执行角色
│   ├── reason TEXT       转换原因 (nullable)
│   └── timestamp TEXT
│
└── pipeline_progress_log 执行进度日志
    ├── id INTEGER PK
    ├── task_id TEXT       → pipeline_tasks.id
    ├── agent_role TEXT    上报角色
    ├── content TEXT       进度描述
    ├── percentage REAL    完成百分比 (nullable)
    └── timestamp TEXT
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
└── pipeline_templates/     管线角色 SOUL 模板 (opt-in)
    ├── taizi.md            太子 — 分诊
    ├── zhongshu.md         中书省 — 规划
    ├── menxia.md           门下省 — 审核
    ├── shangshu.md         尚书省 — 调度
    └── ministry_default.md 六部通用执行
```

> **Bootstrap 文件加载顺序**: PROFILE.md → SOUL.md → AGENTS.md → MEMORY.md → BOOTSTRAP.md
>
> MEMORY.md 有 2048 token 硬限制，超出时自动截断保留尾部（最新内容）。
