# 模块设计

> Nanobot-RS 各模块职责与接口设计

---

## 1. providers/ — LLM 提供商抽象层

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

### 核心 Trait

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    async fn execute(&self, args: Value) -> ToolResult;
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
| `sandbox.rs` | `SandboxProvider` — 沙箱约束 (目录限制) |
| `resource_limits.rs` | 资源限制 (文件大小, 输出长度等) |
| `command_policy.rs` | Shell 命令策略 (白名单/黑名单) |

---

## 3. channels/ — 通信渠道

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

```
┌─────────────┐    JSON-RPC 2.0     ┌──────────────────┐
│  MCP Client │◄───── stdio ───────▶│  MCP Server      │
│  (nanobot)  │                     │  (外部进程)       │
│             │                     │                   │
│  initialize │────────────────────▶│  返回 tool 列表   │
│  tools/list │────────────────────▶│  返回 tool 定义   │
│  tools/call │────────────────────▶│  执行并返回结果   │
└─────────────┘                     └──────────────────┘
```

### 子模块结构

| 文件 | 职责 |
|------|------|
| `client.rs` | `McpClient` — JSON-RPC 2.0 over stdio 通信 |
| `manager.rs` | `McpManager` — 管理多个 MCP 服务器生命周期 |
| `tool.rs` | `McpToolBridge` — 将 MCP 工具适配为 `trait Tool` |
| `types.rs` | `McpServerConfig`, `McpTool` 等类型定义 |

---

## 5. bus/ — 消息总线 (Actor 模型)

### 模块结构

| 文件 | 职责 |
|------|------|
| `events.rs` | 事件类型定义: `ChannelType`, `SessionKey`, `InboundMessage`, `OutboundMessage`, `MediaAttachment` |
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

## 6. hooks/ — 外部 Shell Hook 系统

```
Rust → stdin (JSON) → Shell Script → stdout (JSON) → Rust
                        stderr → tracing::debug!
```

- 脚本位于 `~/.nanobot/hooks/`
- `pre_request.sh` — 请求预处理（可修改或中止输入）
- `post_response.sh` — 响应后处理（审计/告警）
- 2 秒超时，1 MB stdout 上限，非阻塞 `tokio::process::Command`

---

## 7. memory/ — 存储抽象层

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
| `history_processor.rs` | token 感知的历史截断（tiktoken-rs BPE 编码） |
| `prompt.rs` | 系统提示词加载（bootstrap 文件 + 技能 + token 截断保护） |
| `summarization.rs` | `SummarizationService` + `ContextCompressionHook` — LLM 摘要 |
| `stream.rs` | 流式输出累积器 |
| `request.rs` | 请求构建与重试逻辑 |
| `memory.rs` | Agent 工作空间内存管理 |
| `skill_loader.rs` | 技能文件加载器 (Markdown + YAML frontmatter) |
| `subagent.rs` | 子代理管理 (`submit()` 异步 + `submit_and_wait()` 同步 + `submit_tracked()` 追踪 + `submit_tracked_streaming()` 流式) |

### ContextCompressionHook

可扩展的上下文压缩接口，解耦压缩策略与 Agent 循环：

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

当前实现 `SummarizationService`：当历史消息被驱逐时，调用 LLM 生成摘要并持久化到 SQLite。

> **注意**: `ContextCompressionHook` 已简化为 `SummarizationService` 的 `compress()` 方法，不再作为独立 trait。`AgentContext::compress_context()` 直接调用此方法。

---

## 10. config/ — 配置管理

- `loader.rs` — 配置文件加载 (`~/.nanobot/config.yaml`)
- `schema.rs` — 配置结构定义 (providers, agents, channels, tools 等)
- `provider.rs` — Provider 配置定义
- `agent.rs` — Agent 配置定义
- `channel.rs` — 渠道配置定义
- 兼容 Python nanobot 配置格式

---

## 11. vault/ — 敏感数据隔离模块

> 详细使用指南见 [vault-guide.md](vault-guide.md)

### 核心组件

| 文件 | 职责 |
|------|------|
| `store.rs` | `VaultStore` — JSON 文件存储，支持加密 |
| `injector.rs` | `VaultInjector` — 运行时占位符替换 |
| `scanner.rs` | 占位符扫描与解析 (`{{vault:key}}`) |
| `crypto.rs` | `VaultCrypto` — AES-256-GCM 加密 |
| `redaction.rs` | 日志脱敏函数 (`redact_secrets`) |
| `error.rs` | `VaultError` 错误类型 |

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

## 12. search/ — 搜索类型定义

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

> **注意**: 高级 Tantivy 全文搜索已迁移到独立的 `tantivy-mcp` MCP 服务器。

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
| `error.rs` | 统一错误类型定义（AgentError, ProviderError, McpError, ChannelError, PipelineError） |
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
