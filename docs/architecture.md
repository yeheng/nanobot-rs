# 结构设计

> Gasket-RS 系统架构总览

---

## Crate 结构

```
gasket-rs/                    (Cargo workspace)
├── engine/                   核心编排 crate — Agent 引擎、工具、Hook 系统
│   └── src/
│       ├── kernel/            纯函数执行核心 (executor, stream)
│       ├── session/           会话管理层 (AgentSession, context, compactor, memory)
│       ├── subagents/         子代理系统 (manager, tracker, runner)
│       ├── config/            配置加载 (YAML → Struct)
│       ├── cron/              定时任务服务
│       ├── heartbeat/         心跳服务
│       ├── hooks/             Pipeline Hook 系统
│       ├── skills/            技能系统
│       ├── tools/             工具系统 (14 个内置工具)
│       └── vault/             敏感数据隔离模块
├── cli/                      CLI 可执行文件
│   └── src/
│       ├── main.rs            命令入口 + Gateway 启动器
│       ├── cli.rs             CLI 交互模式
│       ├── provider.rs        Provider 工厂
│       └── commands/          子命令 (onboard, status, agent, gateway, channels, cron, vault, memory)
├── types/                    共享类型定义 (Tool trait, events, session_event 等)
├── providers/                LLM 提供商实现
├── storage/                  SQLite 存储 + 记忆系统实现
├── channels/                 通信渠道实现
├── sandbox/                  沙箱执行环境
└── tantivy/                  Tantivy 搜索 MCP 服务器 (独立二进制)
```

---

## 系统架构图

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
│  │                   AgentSession (会话管理)                   │  │
│  │  ┌────────────┐  ┌──────────────┐  ┌──────────────────┐   │  │
│  │  │   Prompt   │  │    kernel    │  │    Session        │   │  │
│  │  │   Loader   │  │   execute    │  │   Management     │   │  │
│  │  └────────────┘  └──────────────┘  └──────────────────┘   │  │
│  │  ┌────────────────────┐  ┌────────────────────────────┐   │  │
│  │  │  Context Compactor │  │      Hook Registry         │   │  │
│  │  │  (同步压缩)        │  │  (BeforeRequest/AfterResp) │   │  │
│  │  └────────────────────┘  └────────────────────────────┘   │  │
│  └──────────┬──────────────┬──────────────────┬──────────────┘  │
│             │              │                  │                  │
│  ┌──────────▼──────┐  ┌───▼──────────┐  ┌───▼──────────────┐  │
│  │  Providers      │  │  Tool        │  │   Memory         │  │
│  │  (re-export)    │  │  Registry    │  │   Manager        │  │
│  │                 │  │              │  │                  │  │
│  │ ┌─────────────┐ │  │ ┌──────────┐ │  │  长期记忆系统     │  │
│  │ │  OpenAI     │ │  │ │Filesystem│ │  │  (Scenario-based)│  │
│  │ │  Compatible │ │  │ │Shell     │ │  └─────────────────┘  │
│  │ │  Provider   │ │  │ │WebSearch │ │                       │
│  │ ├─────────────┤ │  │ │WebFetch  │ │  ┌─────────────────┐  │
│  │ │  Gemini     │ │  │ │Spawn    │ │  │  EventStore     │  │
│  │ │  Provider   │ │  │ │SpawnPar.│ │  │  (SQLite 后端)   │  │
│  │ │             │ │  │ │Message  │ │  │                 │  │
│  │ ├─────────────┤ │  │ │Cron     │ │  │  session_events │  │
│  │ │  Copilot    │ │  │ │MCP Tools│ │  │  memory_metadata│  │
│  │ │  Provider   │ │  │ │Memory   │ │  └─────────────────┘  │
│  │ └─────────────┘ │  │ └──────────┘ │                       │
│  └────────────────┘  └──────────────┘                       │
│                                                             │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  kernel (纯函数执行核心)                                  ││
│  │  ├── executor.rs: AgentExecutor, ToolExecutor            ││
│  │  ├── stream.rs: StreamEvent 流式输出                     ││
│  │  └── context.rs: RuntimeContext, KernelConfig            ││
│  └─────────────────────────────────────────────────────────┘│
│                                                             │
│  ┌─────────────────────────────────────────────────────────┐│
│  │  subagents (子代理系统)                                   ││
│  │  ├── manager.rs: SubagentManager, SubagentTaskBuilder   ││
│  │  ├── tracker.rs: SubagentTracker, 并行任务协调          ││
│  │  └── runner.rs: run_subagent, ModelResolver             ││
│  └─────────────────────────────────────────────────────────┘│
│                                                             │
│  ┌───────────────┐  ┌────────────────┐  ┌──────────────────┐│
│  │  Heartbeat    │  │  Cron Service  │  │  MCP Client      ││
│  │  Service      │  │  (文件驱动：     │  │  (JSON-RPC 2.0)  ││
│  │               │  │   ~/.gasket/   │  │                  ││
│  │               │  │   cron/*.md)   │  │                  ││
│  └───────────────┘  └────────────────┘  └──────────────────┘│
│                                                             │
│  ┌─────────────────────────────────────────────────────────┐│
│  │              Vault (敏感数据隔离模块)                    ││
│  │                                                         ││
│  │  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  ││
│  │  │ VaultStore  │  │ VaultInjector│  │  VaultCrypto  │  ││
│  │  │ (JSON 存储) │  │ (运行时注入) │  │  (XChaCha20)  │  ││
│  │  └─────────────┘  └──────────────┘  └───────────────┘  ││
│  │                                                         ││
│  │  占位符语法: {{vault:key}}                              ││
│  │  日志脱敏: redact_secrets()                             ││
│  └─────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────┘

                    ┌─────────────────────┐
                    │   External LLM APIs  │
                    │  OpenAI / Anthropic  │
                    │  DeepSeek / Gemini   │
                    │  Ollama / Copilot    │
                    └─────────────────────┘
```

### 核心设计原则

| 原则 | 实现方式 |
|------|----------|
| **AgentContext 枚举** | 零成本枚举分发替代 Option<T> 模式，PersistentContext 变体（完整依赖）和 Stateless 变体（无持久化） |
| **Kernel 纯函数设计** | `kernel::execute()` 和 `kernel::execute_streaming()` 无副作用，输入输出清晰 |
| **Session 状态管理** | `AgentSession` 包装 kernel，管理会话状态、提示加载、Hook 注册 |
| **Pipeline Hook 扩展** | 五个执行点（BeforeRequest, AfterHistory, BeforeLLM, AfterToolCall, AfterResponse）支持顺序/并行策略 |
| **Feature Flag 编译** | 各通信渠道通过 Cargo feature flag 独立编译，按需启用 |
| **无内存缓存** | Session 直接读写 SQLite，利用 SQLite page cache 避免缓存一致性问题 |
| **Vault 敏感数据隔离** | 敏感数据与 LLM 可访问存储完全隔离，仅运行时注入，支持加密存储 |
| **模块化 Skills 系统** | 独立的 skills/ 模块，支持 Markdown + YAML frontmatter 格式，渐进式加载 |
| **文件驱动 Cron** | Cron jobs 存储在 ~/.gasket/cron/*.md，notify 监听热重载，无需 SQLite 持久化 |
| **Crate 分离** | 核心类型、提供商、存储、渠道等已拆分为独立 crate |

---

## 模块依赖关系

```
engine
    │
    ├── re-exports from types
    │       └── Tool trait, events (ChannelType, SessionKey, InboundMessage, etc.)
    │       └── SessionEvent, EventType, Session (事件溯源类型)
    │
    ├── re-exports from providers
    │       └── LlmProvider trait, ChatRequest, ChatResponse, etc.
    │
    ├── re-exports from storage (as memory 模块)
    │       └── SqliteStore, EventStore, StoreError, MemoryStore
    │       └── memory 子模块 (MetadataStore, EmbeddingStore 等)
    │
    ├── session/ (会话管理层)
    │       └── AgentSession (原 AgentLoop), AgentContext, ContextCompactor
    │       └── MemoryManager, MemoryProvider trait
    │
    ├── kernel/ (纯函数执行核心)
    │       └── AgentExecutor, ToolExecutor, execute(), execute_streaming()
    │
    ├── subagents/ (子代理系统)
    │       └── SubagentManager, SubagentTracker
    │
    ├── optional: channels (feature flags)
    │       └── Telegram, Discord, Slack, Feishu, Email, DingTalk, WeCom, Webhook, WebSocket
    │
    └── optional: providers (feature flags)
            └── Gemini, Copilot
```

---

## 关键组件说明

### AgentSession (原 AgentLoop)

`AgentSession` 是会话管理的核心结构，负责：

```rust
pub struct AgentSession {
    runtime_ctx: RuntimeContext,    // kernel 执行上下文
    context: AgentContext,          // 持久化/无状态上下文
    config: AgentConfig,            // Agent 配置
    workspace: PathBuf,             // 工作空间路径
    system_prompt: String,          // 系统提示
    skills_context: Option<String>, // 技能上下文
    hooks: Arc<HookRegistry>,       // Hook 注册表
    compactor: Option<Arc<ContextCompactor>>, // 上下文压缩器
    memory_manager: Option<Arc<MemoryManager>>, // 记忆管理器
    indexing_service: Option<Arc<IndexingService>>, // 索引服务
}
```

**关键方法：**
- `process_direct()` — 处理消息并返回响应
- `process_direct_streaming_with_channel()` — 流式处理

### Kernel 执行核心

纯函数设计，无副作用：

```rust
/// 纯函数: 执行 LLM 对话循环
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError>;

/// 纯函数: 流式 LLM 对话循环
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError>;
```

### Cron Service (文件驱动架构)

```rust
pub struct CronService {
    /// In-memory job storage
    jobs: RwLock<HashMap<String, CronJob>>,
    /// Workspace path
    workspace: PathBuf,
    /// File watcher
    watcher: RwLock<Option<RecommendedWatcher>>,
    /// Watcher event receiver
    rx: Mutex<Receiver<Result<Event, notify::Error>>>,
}
```

**职责：**
- 启动时扫描 `~/.gasket/cron/*.md` 文件
- 解析 Markdown + YAML frontmatter 格式
- 通过 `notify` crate 监听文件变化，支持热重载
- 内存中计算和更新 `next_run` 时间

### AgentContext 枚举

零成本枚举分发，编译期替代 `Option<T>` 模式：

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

AgentContext 关键方法:
- `persistent(event_store, sqlite_store) -> Self` — 创建持久化变体
- `is_persistent(&self) -> bool` — 运行时检查变体类型
- `load_session(&self, key) -> Session` — 从事件存储加载会话
- `save_event(&self, event) -> Result` — 追加事件到事件存储
- `get_history(&self, key, branch) -> Vec<SessionEvent>` — 获取分支历史
- `recall_history(&self, key, embedding, top_k) -> Vec<String>` — 语义召回
- `clear_session(&self, key) -> Result` — 清除会话数据

| 变体 | 用途 |
|------|------|
| `Persistent(PersistentContext)` | 主 Agent，完整事件溯源（SQLite） |
| `Stateless` | 子 Agent，无持久化，纯计算 |

### Hook 系统

```rust
pub enum HookPoint {
    BeforeRequest,  // 顺序，可修改/中止
    AfterHistory,   // 顺序，可修改
    BeforeLLM,      // 顺序，最后修改
    AfterToolCall,  // 并行，只读
    AfterResponse,  // 并行，只读
}
```

### Feature Flags

| Crate | Flag | 用途 |
|-------|------|------|
| engine | `local-embedding` | ONNX 嵌入 (fastembed) |
| engine | `telegram` | Telegram 渠道 |
| engine | `discord` | Discord 渠道 |
| engine | `slack` | Slack 渠道 |
| engine | `email` | 邮件渠道 |
| engine | `feishu` | 飞书渠道 |
| engine | `dingtalk` | 钉钉渠道 |
| engine | `wecom` | 企业微信渠道 |
| engine | `webhook` | Webhook 服务器 |
| engine | `provider-gemini` | Google Gemini 提供商 |
| engine | `provider-copilot` | GitHub Copilot 提供商 |
| storage | `local-embedding` | fastembed ONNX 嵌入 (~20MB) |
| cli | `full` | 全部功能 |
| cli | `telemetry` | OpenTelemetry 支持 |

### Actor 模型

| Actor | 职责 | 特点 |
|-------|------|------|
| Router | 按 SessionKey 分发 | 单任务，HashMap 路由表 |
| Session | 串行处理消息 | 每 session 一个，空闲超时自毁 |
| Outbound | HTTP/WebSocket 发送 | 单任务，fire-and-forget 发送 |

---

## 扩展 Crate

| Crate | 用途 | 依赖 |
|-------|------|------|
| `types` | 共享类型定义，最小依赖 | 无 |
| `providers` | LLM 提供商实现 | types, async-trait |
| `storage` | SQLite 存储 + embedding + 记忆系统 | types, sqlx, fastembed |
| `channels` | 通信渠道 | teloxide, serenity, etc. |
| `sandbox` | 沙箱执行 | 系统进程管理 |
| `tantivy` | 全文搜索 MCP 服务器 | tantivy |
