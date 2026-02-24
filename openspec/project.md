# Project Context

## Purpose

nanobot-rs 是一个轻量级个人 AI 助手框架，使用 Rust 编写。项目目标是：

- 提供一个模块化、可扩展的 AI 助手核心库
- 支持多种 LLM 提供商（OpenAI、Gemini、OpenRouter 等）
- 支持多种聊天渠道（Telegram、Discord、Slack、Email、钉钉、飞书、企业微信等）
- 提供工具系统和技能系统，增强 AI 助手能力
- 支持 MCP（Model Context Protocol）协议
- 提供会话管理和记忆持久化功能

## Tech Stack

- **语言**: Rust 2021 Edition
- **异步运行时**: Tokio
- **序列化**: serde, serde_json, serde_yaml
- **HTTP 客户端**: reqwest
- **CLI 框架**: clap
- **日志追踪**: tracing, tracing-subscriber
- **可观测性**: OpenTelemetry
- **错误处理**: thiserror, anyhow
- **数据库**: SQLite (rusqlite)
- **终端渲染**: termimad (Markdown)
- **REPL**: reedline

### 可选依赖（通过 feature flags 启用）

- **Telegram**: teloxide
- **Discord**: serenity
- **Slack**: tokio-tungstenite
- **Email**: lettre, async-imap
- **钉钉**: base64, sha2
- **飞书**: 内置 HTTP API
- **企业微信**: sha1, aes, cbc, quick-xml
- **Webhook**: axum, tower, tower-http

## Project Conventions

### Code Style

- 使用 `rustfmt` 进行代码格式化
- 使用 Clippy 进行代码检查，配置如下：
  - `clippy::all` - warn
  - `clippy::pedantic` - warn
  - `clippy::nursery` - warn
- **禁止 unsafe 代码** (`unsafe_code = "forbid"`)
- 遵循 Rust 标准命名约定（snake_case 函数/变量，PascalCase 类型）

### Architecture Patterns

项目采用 **Workspace** 架构，包含两个 crate：

1. **nanobot-core** - 核心库
   - `agent/` - Agent 循环，处理消息和工具调用
   - `providers/` - LLM 提供商抽象层
   - `tools/` - 工具系统（文件操作、Shell 执行、Web 搜索等）
   - `skills/` - 技能系统，动态加载技能
   - `channels/` - 多渠道集成（Telegram、Discord、Slack 等）
   - `session/` - 会话管理
   - `memory/` - 记忆持久化（SQLite）
   - `config/` - 配置管理
   - `mcp/` - MCP 协议支持
   - `cron/` - 定时任务
   - `heartbeat/` - 心跳服务
   - `webhook/` - Webhook 服务器

2. **nanobot-cli** - 命令行应用
   - 提供 REPL 交互模式
   - 管理 Gateway 服务
   - 管理配置和渠道

### Feature Flags

使用 Cargo features 控制可选功能：

- `telegram` - Telegram 渠道
- `discord` - Discord 渠道
- `slack` - Slack 渠道
- `email` - Email 渠道
- `dingtalk` - 钉钉渠道
- `feishu` - 飞书渠道
- `wecom` - 企业微信渠道
- `webhook` - Webhook 服务器
- `all-channels` - 启用所有渠道
- `markdown` - Markdown 渲染（CLI）

### Testing Strategy

- 使用 `cargo test` 运行单元测试
- 测试文件位于 `tests/` 目录（目前为空）
- 开发依赖包括 `tokio-test`, `dotenvy`, `tempfile`
- 建议为新功能编写单元测试

### Git Workflow

- **主分支**: `main`
- **提交信息**: 遵循 Conventional Commits 规范
  - `feat:` - 新功能
  - `fix:` - Bug 修复
  - `refactor:` - 重构
  - `docs:` - 文档更新
  - `test:` - 测试相关
  - `chore:` - 构建/工具变更

## Domain Context

### AI 助手核心概念

- **Agent Loop**: 处理用户消息，调用 LLM，执行工具，返回响应的循环
- **Provider**: LLM 提供商抽象，支持 OpenAI 兼容 API
- **Tool**: AI 可调用的工具（如读取文件、执行命令）
- **Skill**: 动态加载的技能包，扩展 AI 能力
- **Channel**: 聊天渠道，连接用户和 AI 助手
- **Session**: 用户会话管理
- **Memory**: 对话历史和上下文持久化

### 支持的 LLM 提供商

- OpenAI (GPT-4, GPT-3.5)
- Google Gemini
- OpenRouter (多模型聚合)
- 任何 OpenAI 兼容 API

## Important Constraints

- **许可证**: MIT
- **Rust 版本**: 2021 Edition
- **最低 Rust 版本**: 参考 `Cargo.toml` 中的 `edition`
- **平台支持**: Linux, macOS, Windows
- **异步**: 必须使用 Tokio 运行时
- **TLS**: 使用 rustls（不依赖 OpenSSL）

## External Dependencies

### LLM 提供商

- OpenAI API
- Google Gemini API
- OpenRouter API

### 聊天平台

- Telegram Bot API
- Discord Gateway API
- Slack Webhook/RTM API
- 钉钉机器人 API
- 飞书开放平台 API
- 企业微信 API
- SMTP/IMAP 服务器（Email）

### 可观测性

- OpenTelemetry Collector（可选）
- 支持 OTLP 协议

### MCP 服务器

- 支持 Model Context Protocol 的外部工具服务器
