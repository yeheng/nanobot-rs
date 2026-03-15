# Nanobot-RS

> 版本: 1.0.0 | 语言: Rust (Edition 2021) | 许可: MIT

---

## 项目概览

Nanobot-RS 是一个用 Rust 编写的**轻量级个人 AI 助手框架**。它通过统一的 Agent Loop 连接多种 LLM 提供商和多种通信渠道，支持工具调用、长期记忆、定时任务、子代理和 MCP 协议。

### 核心特性

| 特性 | 说明 |
|------|------|
| 多 LLM 提供商 | OpenRouter, OpenAI, Anthropic, DeepSeek, 智谱, 通义千问, Moonshot, MiniMax, Ollama, Gemini, Copilot |
| 多通信渠道 | CLI, Telegram, Discord, Slack, 飞书, 邮件, 钉钉, 企业微信, WebSocket |
| 工具系统 | 文件读写, Shell 执行, Web 搜索/抓取, 消息发送, 定时任务, 子代理, 记忆搜索 |
| MCP 协议 | 通过 JSON-RPC 2.0 over stdio 连接外部工具服务器 |
| 流式输出 | 支持 SSE 流式响应, 包含 thinking/reasoning 模式 |
| 持久化存储 | SQLite (FTS5 全文搜索) 存储会话历史、结构化记忆、任务状态 |
| 技能系统 | 从 Markdown+YAML frontmatter 文件动态加载技能 |
| Actor 消息模型 | Router → Session → Outbound 三 Actor 零锁流水线 |
| 可扩展 Hook | 外部 Shell 脚本 Hook 系统 (UNIX 哲学) |

### Crate 结构

```
nanobot-rs/                    (Cargo workspace)
├── nanobot-core/              核心库 — 所有业务逻辑
│   └── src/
│       ├── agent/             Agent 核心引擎
│       ├── bus/               消息总线 (Actor 模型)
│       ├── channels/          通信渠道
│       ├── config/            配置加载
│       ├── cron/              定时任务
│       ├── hooks/             外部 Shell Hook
│       ├── mcp/               MCP 协议客户端
│       ├── memory/            存储层 (SQLite + FTS5)
│       ├── providers/         LLM 提供商
│       ├── session/           会话管理
│       ├── skills/            技能加载器
│       ├── tools/             工具系统
│       └── ...
└── nanobot-cli/               CLI 可执行文件
    └── src/                   命令入口 + Gateway 启动器
```

---

## 设计文档

详细的系统设计文档请参阅 `docs/` 目录：

| 文档 | 内容 |
|------|------|
| [结构设计](docs/architecture.md) | 系统架构图、Crate 结构、核心设计原则 |
| [数据结构设计](docs/data-structures.md) | 消息类型、LLM 请求/响应、SQLite 表结构、文件系统存储 |
| [数据流设计](docs/data-flow.md) | CLI/Gateway/Heartbeat/Cron 模式数据流、Agent 执行流程、流式输出 |
| [模块设计](docs/modules.md) | providers, tools, channels, mcp, bus, hooks, memory, session 等模块详解 |
| [Copilot 配置](docs/copilot-setup.md) | GitHub Copilot (PAT/OAuth) 的两种调用方式与设置指南 |

---

## 安装与使用教程

### 前置要求

- Rust 1.75+ (推荐使用 `rustup` 安装)
- SQLite3 (bundled, 无需单独安装)
- 至少一个 LLM API Key

### 构建安装

```bash
# 克隆仓库
git clone https://github.com/YeHeng/nanobot-rs.git
cd nanobot-rs

# 构建 (启用所有渠道)
cargo build --release

# 二进制文件位于
./target/release/nanobot

# 可选: 安装到系统路径
cargo install --path nanobot-cli
```

**仅构建部分渠道:**

```bash
# 不含企业平台
cargo build --release --no-default-features --features "markdown,telegram,discord,slack,email"
```

### 初始化

```bash
# 初始化配置和工作空间
nanobot onboard
```

这会创建:
- `~/.nanobot/config.yaml` — 主配置文件
- `~/.nanobot/PROFILE.md` — Agent 角色定义
- `~/.nanobot/SOUL.md` — Agent 灵魂
- `~/.nanobot/AGENTS.md` — Agent 行为描述
- `~/.nanobot/BOOTSTRAP.md` — 启动引导
- `~/.nanobot/MEMORY.md` — 长期记忆
- `~/.nanobot/hooks/` — Shell Hook 脚本
- `~/.nanobot/memory/` — 记忆目录
- `~/.nanobot/skills/` — 自定义技能目录

### 配置文件

编辑 `~/.nanobot/config.yaml`:

```yaml
# ── LLM 提供商 ─────────────────────────────────
providers:
  openrouter:
    api_key: sk-or-v1-your-key-here
  deepseek:
    api_key: sk-your-deepseek-key
  ollama:
    api_base: http://localhost:11434  # 本地 Ollama, 无需 key

# ── Agent 默认设置 ──────────────────────────────
agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet  # provider/model 格式
    temperature: 0.7
    max_tokens: 4096
    max_iterations: 20
    memory_window: 50
    streaming: true
    thinking_enabled: false  # 对 DeepSeek R1 / GLM-5 等推理模型启用

# ── 通信渠道 ────────────────────────────────────
channels:
  telegram:
    enabled: true
    token: "123456:ABC-DEF"
    allow_from:
      - "your_telegram_user_id"

  discord:
    enabled: false
    token: "your-discord-bot-token"
    allow_from: []

  feishu:
    enabled: false
    app_id: "cli_xxxxx"
    app_secret: "your-app-secret"

  email:
    enabled: false
    imap_host: imap.gmail.com
    imap_port: 993
    imap_username: you@gmail.com
    imap_password: app-password
    smtp_host: smtp.gmail.com
    smtp_port: 587
    smtp_username: you@gmail.com
    smtp_password: app-password
    from_address: you@gmail.com
    allow_from:
      - "trusted@example.com"
    consent_granted: true

# ── 工具配置 ────────────────────────────────────
tools:
  restrict_to_workspace: false  # true = 文件操作限制在 ~/.nanobot 内
  exec:
    timeout: 120  # Shell 命令超时 (秒)
  web:
    search_provider: brave  # brave | tavily | exa | firecrawl
    brave_api_key: BSA-your-key
  mcp_servers:
    weather:
      command: npx
      args: ["-y", "@modelcontextprotocol/server-weather"]
```

### 命令行使用

```bash
# ── 查看状态 ──
nanobot status

# ── 单次对话 ──
nanobot agent -m "用 Rust 写一个快速排序"

# ── 交互模式 (REPL) ──
nanobot agent

# ── 启用推理模式 (DeepSeek R1 / GLM-5) ──
nanobot agent --thinking -m "分析这段代码的时间复杂度"

# ── 禁用流式输出 ──
nanobot agent --no-stream -m "hello"

# ── 禁用 Markdown 渲染 ──
nanobot agent --no-markdown -m "hello"

# ── 显示调试日志 ──
nanobot agent --logs -m "test"

# ── 启动 Gateway (多渠道长驻服务) ──
nanobot gateway

# ── 查看渠道状态 ──
nanobot channels status
```

### 交互模式内置命令

| 命令 | 说明 |
|------|------|
| `/new` | 清空当前会话, 开始新对话 |
| `/help` | 显示可用命令 |
| `/exit`, `/quit`, `exit`, `quit`, `:q` | 退出交互模式 |
| `Ctrl+C`, `Ctrl+D` | 退出 |

### 自定义技能

在 `~/.nanobot/skills/` 目录下创建 Markdown 文件:

```markdown
---
name: code-review
description: 代码审查助手
tags: [code, review]
enabled: true
---

你是一个专业的代码审查助手。请按照以下标准审查代码:

1. 代码风格和一致性
2. 潜在的 Bug 和安全问题
3. 性能优化建议
4. 可读性和可维护性
```

### 环境变量

| 变量 | 说明 |
|------|------|
| `RUST_LOG` | 日志级别 (`debug`, `info`, `warn`, `error`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry 端点 (如 `http://localhost:4317`) |
| `OTEL_SDK_DISABLED=true` | 禁用 OpenTelemetry |

### 典型使用场景

**场景一: 本地开发助手 (CLI)**

```bash
# 使用本地 Ollama
# config.yaml:
#   providers:
#     ollama: {}
#   agents:
#     defaults:
#       model: ollama/llama3

nanobot agent
> 帮我写一个 HTTP 服务器
> /new
> 解释一下 Rust 的生命周期
```

**场景二: Telegram 个人助手**

```bash
# 配置好 telegram token 和 provider 后
nanobot gateway
# Bot 开始监听 Telegram 消息
```

**场景三: 多渠道企业网关**

```bash
# 配置 telegram + 飞书 + 邮件
nanobot gateway
# 同时监听所有渠道, 统一由 Agent 处理
# 支持定时任务 (HEARTBEAT.md) 和 cron
```

---

## 技术栈一览

| 领域 | 技术 |
|------|------|
| 语言 | Rust 2021 Edition |
| 异步运行时 | tokio (full) |
| HTTP 客户端 | reqwest + rustls-tls |
| HTTP 服务器 | axum + tower |
| CLI 框架 | clap 4 (derive) |
| REPL | reedline |
| 序列化 | serde + serde_json + serde_yaml |
| 数据库 | sqlx (SQLite, async) |
| Telegram | teloxide |
| Discord | serenity |
| Slack | tokio-tungstenite (WebSocket) |
| Email | lettre (SMTP) + async-imap (IMAP) |
| 日志/追踪 | tracing + tracing-subscriber |
| 分布式追踪 | OpenTelemetry + OTLP |
| Token 计数 | tiktoken-rs (BPE) |
| Markdown 渲染 | termimad |
| 终端颜色 | colored |
| Cron 解析 | cron 0.15 |

---

## 动态模型切换

Nanobot-RS 支持在运行时动态切换 LLM 模型。主 Agent 作为编排者，可以将特定任务委托给专门优化的子模型执行。

### 配置模型档案

在 `config.yaml` 中定义多个模型档案:

```yaml
agents:
  defaults:
    model: "zhipu/glm-5"  # 默认模型

  # 模型档案配置
  models:
    # 通用模型
    default:
      provider: "zhipu"
      model: "glm-5"
      description: "通用模型，平衡速度与质量"
      capabilities: ["general", "chat"]
      temperature: 0.7

    # 快速响应模型
    fast:
      provider: "zhipu"
      model: "glm-4-flash"
      description: "快速响应简单查询"
      capabilities: ["fast", "chat"]
      temperature: 0.7

    # 代码专家模型
    coder:
      provider: "deepseek"
      model: "deepseek-coder"
      description: "专门用于代码生成、重构和调试"
      capabilities: ["code", "reasoning"]
      temperature: 0.3
      thinking_enabled: true

    # 推理模型
    reasoner:
      provider: "openai"
      model: "o1-mini"
      description: "复杂问题推理、数学和逻辑分析"
      capabilities: ["reasoning", "math", "analysis"]
      thinking_enabled: true

    # 创意写作模型
    creative:
      provider: "anthropic"
      model: "claude-sonnet-4-20250514"
      description: "创意写作、内容生成"
      capabilities: ["creative", "writing"]
      temperature: 0.9
```

### 能力标签

使用能力标签帮助 LLM 选择合适的模型:

| 标签 | 用途 |
|------|------|
| `code` | 代码生成、重构、调试 |
| `reasoning` | 复杂逻辑推理 |
| `creative` | 创意写作、内容生成 |
| `fast` | 快速响应简单任务 |
| `math` | 数学计算 |
| `research` | 深度研究和分析 |
| `local` | 本地模型，隐私保护 |

### 使用示例

LLM 会根据任务类型自动选择合适的模型:

```json
// 用户: "帮我重构这个函数，使其更高效"
// LLM 调用 switch_model 工具:
{
  "model_id": "coder",
  "task": "重构用户提供的函数，优化性能",
  "context": "原函数在 src/lib.rs 第 42 行"
}
```

### 工作原理

1. **主 Agent**: 负责理解用户意图、编排任务
2. **switch_model 工具**: LLM 可调用的工具，用于切换到专门模型
3. **Subagent**: 在隔离环境中以目标模型执行任务
4. **结果返回**: 子 Agent 的结果返回给主 Agent 继续处理

这种架构允许:
- 使用便宜的快速模型处理简单任务
- 将复杂任务委托给专门优化的模型
- 主 Agent 保持轻量，专注于编排
- 灵活组合不同提供商的模型
