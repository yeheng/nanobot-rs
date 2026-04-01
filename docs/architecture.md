# 结构设计

> Gasket-RS 系统架构总览

---

## Crate 结构

```
gasket-rs/                    (Cargo workspace)
├── engine/                   核心编排 crate — Agent 引擎、工具、Hook 系统
│   └── src/
│       ├── agent/             Agent 核心引擎 (loop, executor, prompt, history, stream, summarization, subagent, context)
│       ├── bus/               消息总线 (Actor 模型: Router/Session/Outbound)
│       ├── channels/          通信渠道 re-export (从 channels)
│       ├── config/            配置加载 (YAML → Struct)
│       ├── cron/              定时任务服务
│       ├── crypto/            加密工具
│       ├── heartbeat/         心跳服务
│       ├── hooks/             Pipeline Hook 系统 (BeforeRequest, AfterResponse, etc.)
│       ├── memory/            存储层 re-export (从 storage)
│       ├── providers/         LLM 提供商 re-export (从 providers)
│       ├── session/           会话管理 (SQLite 后端)
│       ├── skills/            技能系统 (loader, registry, skill, metadata)
│       ├── tools/             工具系统 (12 个内置工具)
│       ├── vault/             敏感数据隔离 re-export (从 vault)
│       ├── webhook/           Webhook 服务器
│       └── workspace/         工作空间模板文件
├── cli/                      CLI 可执行文件
│   └── src/
│       ├── main.rs            命令入口 + Gateway 启动器
│       ├── cli.rs             CLI 交互模式
│       ├── provider.rs        Provider 工厂
│       └── commands/          子命令 (onboard, status, agent, gateway, channels, cron, vault)
├── types/                    共享类型定义 (Tool trait, events 等)
├── providers/                LLM 提供商实现
├── storage/                  SQLite 存储实现
├── vault/                    Vault 敏感数据管理
├── channels/                 通信渠道实现
├── sandbox/                  沙箱执行环境
├── bus/                      消息总线 Actor 实现
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
│  │                      Agent Loop (核心引擎)                  │  │
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
│  │                 │  │              │  │   (SQLite 后端)   │  │
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
│  └────────────────┘  │ │Sandbox  │ │  │  │ cron_jobs   │  │  │
│                      │ └──────────┘ │  │  └─────────────┘  │  │
│  ┌────────────────┐  └──────────────┘  └───────────────────┘  │
│  │  Message Bus   │                                            │
│  │  (Actor 模型)  │                                            │
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
│  │  │飞书  │ │ 邮件  │ │ 钉钉  │  │  └───────────────────┘  │
│  │  ├──────┤ ├───────┤ ├────────┤  │                         │
│  │  │企业  │ │WebSoc-│ │  CLI   │  │  ┌───────────────────┐  │
│  │  │微信  │ │ket    │ │       │  │  │   Skills Loader   │  │
│  │  └──────┘ └───────┘ └────────┘  │  │   (MD → Context)  │  │
│  └─────────────────────────────────┘  └───────────────────┘  │
│                                                               │
│  ┌───────────────┐  ┌────────────────┐  ┌──────────────────┐ │
│  │  Heartbeat    │  │  Cron Service  │  │  MCP Client      │ │
│  │  Service      │  │  (定时任务)     │  │  (JSON-RPC 2.0)  │ │
│  └───────────────┘  └────────────────┘  └──────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Vault (敏感数据隔离模块)                    │  │
│  │              (re-export from vault crate)               │  │
│  │                                                         │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │  │
│  │  │ VaultStore  │  │ VaultInjector│  │  VaultCrypto  │  │  │
│  │  │ (JSON 存储) │  │ (运行时注入) │  │  (AES-GCM)    │  │  │
│  │  └─────────────┘  └──────────────┘  └───────────────┘  │  │
│  │                                                         │  │
│  │  占位符语法: {{vault:key}}                              │  │
│  │  日志脱敏: redact_secrets()                             │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Search/Embedding (搜索/嵌入模块)            │  │
│  │              (from storage crate with local-embedding)  │  │
│  │                                                         │  │
│  │  SearchQuery: BooleanQuery, FuzzyQuery, DateRange       │  │
│  │  SearchResult: HighlightedText                          │  │
│  │  TextEmbedder, cosine_similarity                        │  │
│  │  注: 高级 Tantivy 全文搜索在独立的 tantivy crate         │  │
│  └─────────────────────────────────────────────────────────┘  │
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
| **AgentContext trait** | 通过 trait 抽象替代 Option<T> 模式，支持 PersistentContext（完整依赖）和 StatelessContext（无持久化）两种实现 |
| **Actor 模型消息传递** | Gateway 使用三个 Actor（Router → Session → Outbound）通过 mpsc channel 通信，零锁设计 |
| **Pipeline Hook 扩展** | 五个执行点（BeforeRequest, AfterHistory, BeforeLLM, AfterToolCall, AfterResponse）支持顺序/并行策略 |
| **Feature Flag 编译** | 各通信渠道通过 Cargo feature flag 独立编译，按需启用 |
| **无内存缓存** | SessionManager 直接读写 SQLite，利用 SQLite page cache 避免缓存一致性问题 |
| **Vault 敏感数据隔离** | 敏感数据与 LLM 可访问存储完全隔离，仅运行时注入，支持加密存储 |
| **模块化 Skills 系统** | 独立的 skills/ 模块，支持 Markdown + YAML frontmatter 格式，渐进式加载 |
| **Crate 分离** | 核心类型、提供商、存储、Vault、渠道等已拆分为独立 crate，通过 re-export 保持兼容性 |

---

## 模块依赖关系

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
    ├── optional: channels (feature flags)
    │       └── Telegram, Discord, Slack, etc.
    │
    └── optional: mcp (feature flags)
            └── MCP client, manager
```

---

## 关键组件说明

### AgentContext Trait

核心抽象，消除 `Option<T>` 运行时检查：

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

| 实现 | 用途 |
|------|------|
| `PersistentContext` | 主 Agent，完整持久化 |
| `StatelessContext` | 子 Agent，无持久化 |

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
| `storage` | SQLite 存储 + embedding | types, sqlx, fastembed |
| `vault` | Vault 加密存储 | AES-GCM, Argon2 |
| `channels` | 通信渠道 | teloxide, serenity, etc. |
| `sandbox` | 沙箱执行 | sandbox |
| `tantivy` | 全文搜索 MCP 服务器 | tantivy |
