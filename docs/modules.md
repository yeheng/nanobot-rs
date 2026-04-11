# 模块设计

> Gasket-RS 各模块职责与接口设计

---

## 1. providers/ — LLM 提供商抽象层

> **注意**: 核心类型从 `providers` crate re-export，保持向后兼容。

### 核心 Trait

```rust
#[async_trait]
trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream>;
}
```

### 提供商实现

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

- **OpenAICompatibleProvider**: 通过 `PROVIDER_DEFAULTS` 数据表配置，新增提供商只需加一行数据，不需要写代码
- **GeminiProvider**: Google Gemini API（非 OpenAI 兼容格式）
- **CopilotProvider**: GitHub Copilot API（带 OAuth 认证流程）

**ModelSpec 解析格式**: `provider_id/model_id` 或 `model_id`

| 输入 | provider | model |
|------|----------|-------|
| `deepseek/deepseek-chat` | `deepseek` | `deepseek-chat` |
| `anthropic/claude-4.5-sonnet` | `anthropic` | `claude-4.5-sonnet` |
| `gpt-4o` | `None` (使用默认) | `gpt-4o` |

---

## 2. tools/ — 工具系统

> **注意**: `Tool` trait 和基础类型从 `types` re-export，沙箱类型从 `sandbox` re-export。

### 核心 Trait

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;
}
```

### 内置工具清单

| 工具 | 类别 | 说明 |
|------|------|------|
| `read_file` | filesystem | 读取文件内容 |
| `write_file` | filesystem | 写入文件 |
| `edit_file` | filesystem | 编辑文件 (search/replace) |
| `list_dir` | filesystem | 列出目录内容 |
| `exec` | system | 执行 Shell 命令 (带超时 + command_policy) |
| `spawn` | system | 创建子代理执行任务 |
| `spawn_parallel` | system | 并行创建多个子代理 |
| `web_fetch` | web | HTTP GET 请求 |
| `web_search` | web | Web 搜索 (Brave/Tavily/Exa/Firecrawl) |
| `send_message` | communication | 通过 Bus 发消息到渠道 |
| `cron` | system | 管理定时任务 (CRUD) |
| `memory_search` | memory | 通过 SQLite MetadataStore 搜索结构化记忆 |
| `memorize` | memory | 写入结构化长期记忆 |
| MCP tools | mcp | MCP 服务器提供的动态工具 |

### 辅助模块

| 模块 | 说明 |
|------|------|
| `registry.rs` | `ToolRegistry` — 工具注册表，管理所有可用工具 |
| `base.rs` | 工具基础类型和辅助函数 |

> **注意**: 沙箱相关类型（`ProcessManager`, `SandboxConfig`）从 `sandbox` crate re-export。

---

## 3. channels/ — 通信渠道

> **注意**: `Channel` trait 和相关类型从 `channels` crate 定义，核心代码通过 feature flag 条件编译集成。

### 核心 Trait

```rust
#[async_trait]
trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&mut self) -> Result<()>;  // 开始接收消息
    async fn stop(&mut self) -> Result<()>;   // 停止
    async fn graceful_shutdown(&mut self) -> Result<()>;
}
```

> Channel 是**仅入站**的：接收外部消息并推送到内部 Bus。所有**出站**发送由 Outbound Actor 通过 `send_outbound()` 函数按渠道类型路由处理。

### 渠道列表

| 渠道 | Feature Flag | 传输协议 | 说明 |
|------|-------------|----------|------|
| Telegram | `telegram` | Long Polling (teloxide) | Telegram Bot API |
| Discord | `discord` | WebSocket (serenity) | Discord Gateway |
| Slack | `slack` | WebSocket (tungstenite) | Slack Socket Mode |
| 飞书 | `feishu` | HTTP Webhook (axum) | 飞书事件订阅 |
| 邮件 | `email` | IMAP Polling + SMTP | 邮件收发 |
| 钉钉 | `dingtalk` | HTTP Webhook (axum) | 钉钉回调 |
| 企业微信 | `wecom` | HTTP Webhook (axum) | 企微回调 |
| WebSocket | `webhook` | WebSocket (axum) | 实时双向通信 |

### middleware 层

| 组件 | 说明 |
|------|------|
| `SimpleAuthChecker` | 基于白名单的发送者认证 |
| `SimpleRateLimiter` | 简单速率限制 |
| `InboundSender` | 封装入站消息发送逻辑 |
| `log_inbound` | 入站消息日志记录 |

---

## 4. mcp/ — Model Context Protocol

> **注意**: MCP 功能内嵌在 `engine` crate，通过条件编译集成。

```
┌─────────────┐    JSON-RPC 2.0     ┌──────────────────┐
│  MCP Client │◄───── stdio ───────▶│  MCP Server      │
│  (gasket)  │                     │  (外部进程)       │
│             │                     │                   │
│  initialize │────────────────────▶│  返回 tool 列表   │
│  tools/list │────────────────────▶│  返回 tool 定义   │
│  tools/call │────────────────────▶│  执行并返回结果   │
└─────────────┘                     └──────────────────┘
```

### 核心组件

| 组件 | 职责 |
|------|------|
| `McpClient` | JSON-RPC 2.0 over stdio 通信 |
| `McpManager` | 管理多个 MCP 服务器生命周期 |
| `McpToolBridge` | 将 MCP 工具适配为 `trait Tool` |
| `McpServerConfig` | MCP 服务器配置 |

---

## 5. bus/ — 消息总线 (Actor 模型)

### 模块结构

| 文件 | 职责 |
|------|------|
| `events.rs` | 从 `types` re-export 事件类型: `ChannelType`, `SessionKey`, `InboundMessage`, `OutboundMessage`, `MediaAttachment` |
| `actors.rs` | 三个 Actor: `run_router_actor`, `run_session_actor`, `run_outbound_actor` |
| `queue.rs` | 消息队列封装 |

### Actor 流水线

```
Inbound → [Router Actor] → per-session channel → [Session Actor] → [Outbound Actor] → HTTP
```

- **Router Actor**: 拥有路由表 `HashMap<SessionKey, Sender>`，按 session 分发，懒创建/清理
- **Session Actor**: 串行处理单 session 消息，共享 `Arc<AgentSession>`，空闲超时自毁
- **Outbound Actor**: 专职网络发送，隔离外部 API 延迟

---

## 6. hooks/ — Agent Pipeline 生命周期 Hook 系统

Hook 系统提供统一的管道扩展机制，支持在 Agent 执行流程的关键节点插入自定义逻辑。

### Hook 执行点

| Hook Point | 执行时机 | 执行策略 | 说明 |
|------------|----------|----------|------|
| `BeforeRequest` | 请求处理前 | Sequential | 可修改输入，可中止请求 |
| `AfterHistory` | 历史加载后 | Sequential | 可添加上下文 |
| `BeforeLLM` | 发送给 LLM 前 | Sequential | 最后修改机会 |
| `AfterToolCall` | 工具调用后 | Parallel | 只读访问，fire-and-forget |
| `AfterResponse` | 响应生成后 | Parallel | 审计/告警 |

### 核心组件

| 组件 | 职责 |
|------|------|
| `HookRegistry` | Hook 注册表，按执行点管理所有 Hook |
| `PipelineHook` | Hook trait（`name()`, `point()`, `run()`, `run_parallel()`） |
| `HookBuilder` | HookRegistry 的 Builder 模式创建器 |
| `HookContext<M>` | 泛型上下文（session_key, messages, user_input, response） |
| `ExternalShellHook` | Shell 脚本 Hook 封装 |
| `HistoryRecallHook` | 语义历史召回 (feature: local-embedding) |
| `VaultHook` | BeforeLLM 阶段的 Vault 密钥注入 |

### 外部 Shell Hook

```
Rust → stdin (JSON) → Shell Script → stdout (JSON) → Rust
                        stderr → tracing::debug!
```

- 脚本位于 `~/.gasket/hooks/`
- `pre_request.sh` — 请求预处理（可修改或中止输入）
- `post_response.sh` — 响应后处理（审计/告警）
- 2 秒超时，1 MB stdout 上限，非阻塞 `tokio::process::Command`

---

## 7. memory/ — 存储抽象层

> **注意**: 实际实现从 `storage` crate re-export。

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

### SqliteStore 实现

- 使用 `sqlx::SqlitePool` 原生异步 I/O
- FTS5 全文搜索支持
- 子模块: `memories.rs` (FTS5), `session.rs` (会话持久化), `kv.rs` (键值存储), `cron.rs` (定时任务)

---

## 8. session/ — 会话管理（事件溯源）

> **注意**: 事件溯源类型定义在 `types` crate（`SessionEvent`, `EventType`, `Session`），持久化在 `storage` crate（`EventStore`）。

### 核心类型

| 类型 | 说明 |
|------|------|
| `Session` | 聚合根，管理元数据（created_at, updated_at, total_events） |
| `SessionEvent` | 不可变事件，UUID v7，含 session_key, event_type, content, 可选 embedding |
| `EventType` | UserMessage, AssistantMessage, ToolCall, ToolResult, Summary |
| `SummaryType` | TimeWindow, Topic, Compression |
| `EventMetadata` | branch, tools_used, token_usage, content_token_len, extra |
| `SessionMetadata` | created_at, updated_at, last_consolidated_event, total_events, total_tokens |

### 架构特点

- **事件溯源**: 所有消息以不可变事件存储，支持完整历史重建
- **EventStore** (storage crate): `append_event()`, `get_branch_history()`, `get_events_by_ids()`, `clear_session()`, `get_latest_summary()`
- **纯 SQLite**: 无内存缓存，直接查询数据库，利用 SQLite page cache
- **历史处理**: `process_history()` 基于 token budget, recent_keep, max_events 配置
- **查询系统**: `HistoryQueryBuilder` 支持 branch, time_range, event_types, semantic_query, tools 过滤

---

## 9. session/ — 会话管理（原 agent/）

| 文件 | 职责 |
|------|------|
| `mod.rs` | `AgentSession` — 会话管理核心，包装 kernel 执行 |
| `config.rs` | `AgentConfig` — Agent 配置（含 kernel 转换支持） |
| `context.rs` | `AgentContext` 枚举 — 零成本枚举分发（Persistent/Stateless） |
| `compactor.rs` | `ContextCompactor` — 同步上下文压缩 |
| `memory.rs` | `MemoryManager`, `MemoryContext`, `MemoryProvider` — 记忆管理 |
| `prompt.rs` | 引导文件加载、技能上下文、token 截断 |
| `store.rs` | `MemoryStore` — 内存存储包装器 |

### AgentSession

`AgentSession` 是会话管理的核心结构，包装 kernel 执行：

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

### AgentContext 枚举

```rust
pub enum AgentContext {
    Persistent(PersistentContext),  // 主 Agent，完整事件溯源
    Stateless,                      // 子 Agent，无持久化
}

pub struct PersistentContext {
    pub event_store: Arc<EventStore>,
    pub sqlite_store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    pub embedder: Option<Arc<TextEmbedder>>,
}
```

---

## 10. kernel/ — 纯函数执行核心

| 文件 | 职责 |
|------|------|
| `mod.rs` | `execute()`, `execute_streaming()` — 纯函数执行入口 |
| `executor.rs` | `AgentExecutor`, `ToolExecutor`, `ExecutionResult` — 执行器实现 |
| `context.rs` | `RuntimeContext`, `KernelConfig` — 运行时上下文和配置 |
| `stream.rs` | `StreamEvent`, `BufferedEvents` — 流式输出事件 |
| `error.rs` | `KernelError` — 内核错误类型 |

### 纯函数执行接口

```rust
/// 执行 LLM 对话循环
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError>;

/// 流式 LLM 对话循环
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError>;
```

---

## 11. subagents/ — 子代理系统

| 文件 | 职责 |
|------|------|
| `manager.rs` | `SubagentManager`, `SubagentTaskBuilder` — Builder 模式子代理管理 |
| `tracker.rs` | `SubagentTracker`, `TrackerError` — 并行任务协调 |
| `runner.rs` | `run_subagent()`, `ModelResolver` — 子代理运行和模型解析 |

### SubagentManager API

Builder 模式的任务创建：

```rust
let task_id = manager
    .task("sub-1", "执行任务")
    .with_provider(provider)
    .with_config(config)
    .with_system_prompt("自定义提示词".to_string())
    .with_streaming(event_tx)
    .with_session_key(session_key)
    .with_cancellation_token(token)
    .with_hooks(hooks)
    .spawn(result_tx)
    .await?;
```

---

## 12. config/ — 配置管理

| 文件 | 职责 |
|------|------|
| `mod.rs` | 配置模块导出 |
| `app_config.rs` | 主 `Config` 结构，`ConfigLoader`, `ModelConfig`, `ModelProfile`, `ModelRegistry`, `ProviderConfig`, `ProviderRegistry`, `ProviderType` |
| `tools.rs` | `ToolsConfig`, `ExecToolConfig`（命令策略）, `WebToolsConfig`（搜索/代理）, `SandboxConfig`, `CommandPolicyConfig`, `ResourceLimitsConfig`, `EmbeddingConfig` |

- 配置文件位于 `~/.gasket/config.yaml`
- 兼容 Python gasket 配置格式

---

## 13. vault/ — 敏感数据隔离模块（engine 内部）

> 详细使用指南见 [vault-guide.md](vault-guide.md)

Vault 模块位于 `engine/src/vault/`，不是独立 crate。

### 核心组件

| 类型 | 职责 |
|------|------|
| `VaultStore` | JSON 文件存储，支持加密 |
| `VaultInjector` | 运行时占位符替换（在 `injector.rs` 中定义） |
| `VaultCrypto` | XChaCha20-Poly1305 加密 |
| `Placeholder` | 占位符扫描与解析 (`{{vault:key}}`) |
| `redact_secrets` | 日志脱敏函数 |
| `VaultError` | 错误类型 |

### 设计原则

1. **数据结构隔离** — VaultStore 完全独立于 memory/history 存储
2. **运行时注入** — 敏感数据仅在发送给 LLM 前一刻注入
3. **零信任设计** — 敏感数据永不持久化到 LLM 可访问的存储

### 占位符语法

```
使用 {{vault:api_key}} 来访问 API
密码: {{vault:db_password}}
```

---

## 14. search/ — 搜索与嵌入

> **注意**: 搜索类型从 `storage` crate re-export。高级 Tantivy 全文搜索在独立 `tantivy` crate。

### 核心类型

| 类型 | 说明 |
|------|------|
| `TextEmbedder` | 基于 ONNX 的文本嵌入（fastembed，feature: local-embedding） |
| `EmbeddingConfig` | 模型名称、缓存目录、本地模型路径配置 |
| `cosine_similarity()` | 计算两个向量的余弦相似度 |
| `top_k_similar()` | 从向量集合中获取 Top-K 最相似项 |
| `bytes_to_embedding()` | 字节切片转嵌入向量 |
| `embedding_to_bytes()` | 嵌入向量转字节切片 |

### 语义搜索流程

1. `TextEmbedder::embed(text) -> Vec<f32>` — 为查询生成嵌入
2. `cosine_similarity(query, candidate) -> f32` — 计算相似度分数
3. `top_k_similar(query, vectors, k) -> Vec<(f32, String)>` — 排序结果

---

## 15. 其他模块

| 模块 | 说明 |
|------|------|
| `cron/` | `CronService` + `CronJob` — 定时任务服务，文件驱动 |
| `heartbeat/` | `HeartbeatService` — 读取 HEARTBEAT.md，定时触发主动任务 |
| `skills/` | 技能系统 — `SkillsLoader`, `SkillsRegistry`, `Skill`, `SkillMetadata`（见第 16 节） |
| `bus_adapter.rs` | `EngineHandler` — 桥接引擎到 Bus Actor 系统 |
| `error.rs` | 统一错误类型（AgentError, ProviderError, ChannelError, PipelineError, ConfigValidationError） |
| `token_tracker.rs` | Token 计数、成本计算、会话统计追踪 |

---

## 16. skills/ — 技能系统

### 模块结构

| 文件 | 职责 |
|------|------|
| `loader.rs` | `SkillsLoader` — 从 Markdown 文件加载技能 |
| `registry.rs` | `SkillsRegistry` — 技能注册表管理 |
| `skill.rs` | `Skill` — 技能定义结构 |
| `metadata.rs` | `SkillMetadata` — 技能元数据（name, description, bins, env_vars, always, extra） |

### 技能文件格式

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

技能的详细说明和使用方法...
```

### 加载模式

- **always_load: true** — 启动时自动加载
- **always_load: false** — 按需加载
