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
| `message` | communication | 通过 Bus 发消息到渠道 |
| `cron` | system | 管理定时任务 (CRUD) |
| `memory_search` | memory | 搜索结构化记忆 (FTS5) |
| `history_search` | memory | 搜索会话历史 |
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
- **Session Actor**: 串行处理单 session 消息，共享 `Arc<AgentLoop>`，空闲超时自毁
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
| `HookRegistry` | Hook 注册表，管理所有 Hook |
| `PipelineHook` | Hook trait 定义 |
| `ExternalShellHook` | 外部 Shell 脚本 Hook 实现 |
| `HistoryRecallHook` | 历史记忆召回 Hook |
| `VaultHook` | Vault 注入 Hook |

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

## 8. session/ — 会话管理

**SessionManager**: 纯 SQLite 后端，无内存缓存

- 每次读取直接查询 SQLite，消除缓存一致性问题
- 利用 SQLite page cache 保证读取性能
- 支持 legacy JSON blob 会话自动迁移
- 消息独立存储 (O(1) 追加)

---

## 9. agent/ — Agent 核心引擎

| 文件 | 职责 |
|------|------|
| `loop_.rs` | `AgentLoop` — 核心处理循环，编排所有组件 |
| `executor.rs` | `ToolExecutor` — 工具调用执行（支持并行批量执行） |
| `executor_core.rs` | `AgentExecutor` — 核心执行引擎（新） |
| `history_processor.rs` | token 感知的历史截断（tiktoken-rs BPE 编码） |
| `prompt.rs` | 系统提示词加载（bootstrap 文件 + 技能 + token 截断保护） |
| `summarization.rs` | `SummarizationService` — LLM 摘要生成 |
| `stream.rs` | 流式输出累积器，`StreamEvent` 定义 |
| `request.rs` | 请求构建与重试逻辑 |
| `memory.rs` | Agent 工作空间内存管理 |
| `skill_loader.rs` | 技能文件加载器 (Markdown + YAML frontmatter) |
| `context.rs` | `AgentContext` trait，`PersistentContext` 和 `StatelessContext` 实现 |
| `subagent.rs` | `SubagentManager` — 子代理管理（Builder 模式 API） |
| `subagent_tracker.rs` | `SubagentTracker` — 并行子代理追踪 |
| `spawn_parallel.rs` | `SpawnParallelTool` — 并行子代理工具 |

### AgentContext Trait

通过 trait 抽象替代 `Option<T>` 模式，支持两种实现：

```rust
#[async_trait]
pub trait AgentContext: Send + Sync {
    async fn load_session(&self, key: &SessionKey) -> Session;
    async fn save_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
        tools: Option<Vec<String>>,
    ) -> Result<(), AgentError>;
    async fn load_summary(&self, key: &str) -> Option<String>;
    fn compress_context(&self, key: &str, evicted: &[SessionMessage]);
    async fn recall_history(&self, key: &str, query_embedding: &[f32], top_k: usize)
        -> Result<Vec<String>>;
    fn is_persistent(&self) -> bool;
}
```

- **PersistentContext**: 完整持久化支持（主 Agent）
- **StatelessContext**: 无持久化实现（子 Agent）

### 上下文压缩

`SummarizationService` 提供上下文压缩功能：

```rust
pub async fn compress(&self, session_key: &str, evicted_messages: &[SessionMessage])
    -> Result<()>
```

当历史消息被驱逐时，调用 LLM 生成摘要并持久化到 SQLite。

### SubagentManager API

SubagentManager 提供 Builder 模式的任务创建 API：

```rust
// Builder 模式
let task_id = manager
    .task("sub-1", "执行任务")
    .with_system_prompt("自定义提示词".to_string())
    .with_streaming(event_tx)
    .with_cancellation_token(token)
    .spawn(result_tx)
    .await?;

// 传统 API
manager.submit(prompt, channel, chat_id)?;
manager.submit_and_wait(prompt, system_prompt, channel, chat_id).await?;
```

---

## 10. config/ — 配置管理

- `loader.rs` — 配置文件加载 (`~/.gasket/config.yaml`)
- `schema.rs` — 配置结构定义 (providers, agents, channels, tools 等)
- `provider.rs` — Provider 配置定义
- `agent.rs` — Agent 配置定义
- `channel.rs` — 渠道配置定义
- `tools.rs` — 工具配置定义
- `embedding.rs` — Embedding 配置定义
- `model_registry.rs` — 模型注册表配置
- 兼容 Python gasket 配置格式

---

## 11. vault/ — 敏感数据隔离模块

> 详细使用指南见 [vault-guide.md](vault-guide.md)
> **注意**: 核心类型从 `vault` crate re-export。

### 核心组件

| 类型 | 职责 |
|------|------|
| `VaultStore` | JSON 文件存储，支持加密 |
| `VaultInjector` | 运行时占位符替换（在 `injector.rs` 中定义） |
| `VaultCrypto` | AES-256-GCM 加密 |
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

## 12. search/ — 搜索与嵌入

> **注意**: 搜索功能在 `storage` crate 中实现（`local-embedding` feature）。

### 核心类型

```rust
// 搜索查询
pub enum SearchQuery {
    Boolean(BooleanQuery),
    Fuzzy(FuzzyQuery),
    DateRange(DateRange),
}

// 搜索结果
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub highlights: Vec<HighlightedText>,
}

pub struct HighlightedText {
    pub field: String,
    pub text: String,
    pub highlights: Vec<(usize, usize)>, // 高亮范围
}
```

### 语义搜索

从 `storage` crate:

- `TextEmbedder` — 文本嵌入生成 (fastembed)
- `cosine_similarity` — 余弦相似度计算
- `top_k_similar` — Top-K 相似向量检索

> **注意**: 高级 Tantivy 全文搜索在独立的 `tantivy` crate 中。

---

## 13. 其他模块

| 模块 | 说明 |
|------|------|
| `cron/` | 定时任务服务，每 60 秒检查到期任务 |
| `heartbeat/` | 心跳服务，读取 HEARTBEAT.md 定时触发 |
| `crypto/` | 加密工具（企业微信等渠道需要的消息加解密） |
| `skills/` | 技能系统 (详见下方) |
| `webhook/` | Webhook HTTP 服务器（axum） |
| `workspace/` | 工作空间模板文件（初始化时复制） |
| `error.rs` | 统一错误类型定义（AgentError, ProviderError, ChannelError, PipelineError, ConfigValidationError） |
| `token_tracker.rs` | Token 计数与追踪 |

---

## 14. skills/ — 技能系统

### 模块结构

| 文件 | 职责 |
|------|------|
| `loader.rs` | `SkillsLoader` — 从 Markdown 文件加载技能 |
| `registry.rs` | `SkillsRegistry` — 技能注册表管理 |
| `skill.rs` | `Skill` — 技能定义结构 |
| `metadata.rs` | `SkillMetadata` — 技能元数据（依赖、标签等） |

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
