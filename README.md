# Nanobot-RS 设计文档

> 版本: 1.0.0 | 语言: Rust (Edition 2021) | 许可: MIT

---

## 目录

1. [项目概览](#1-项目概览)
2. [系统架构设计图](#2-系统架构设计图)
3. [数据流动图](#3-数据流动图)
4. [Agent 执行流程图](#4-agent-执行流程图)
5. [存储架构图](#5-存储架构图)
6. [模块详解](#6-模块详解)
7. [关键数据结构](#7-关键数据结构)
8. [安装与使用教程](#8-安装与使用教程)

---

## 1. 项目概览

Nanobot-RS 是一个用 Rust 编写的**轻量级个人 AI 助手框架**。它通过统一的 Agent Loop 连接多种 LLM 提供商和多种通信渠道，支持工具调用、长期记忆、定时任务、子代理和 MCP 协议。

### 核心特性

| 特性 | 说明 |
|------|------|
| 多 LLM 提供商 | OpenRouter, OpenAI, Anthropic, DeepSeek, 智谱, 通义千问, Moonshot, MiniMax, Ollama, Gemini |
| 多通信渠道 | CLI, Telegram, Discord, Slack, 飞书, 邮件, 钉钉, 企业微信 |
| 工具系统 | 文件读写, Shell 执行, Web 搜索/抓取, 消息发送, 定时任务, 子代理 |
| MCP 协议 | 通过 JSON-RPC 2.0 over stdio 连接外部工具服务器 |
| 流式输出 | 支持 SSE 流式响应, 包含 thinking/reasoning 模式 |
| 持久化存储 | SQLite 存储会话历史、长期记忆、任务状态 |
| 技能系统 | 从 Markdown+YAML frontmatter 文件动态加载技能 |

### Crate 结构

```
nanobot-rs/                (Cargo workspace)
├── nanobot-core/          核心库 — 所有业务逻辑
│   ├── src/               84 个 Rust 源文件
│   ├── workspace/         模板文件 (AGENTS.md, SOUL.md, ...)
│   └── skills/            内置技能定义
└── nanobot-cli/           CLI 可执行文件
    └── src/main.rs        命令入口 + Gateway 启动器
```

---

## 2. 系统架构设计图

```
┌──────────────────────────────────────────────────────────────────┐
│                        nanobot-cli (Binary)                      │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌──────────┐ ┌─────────┐ │
│  │ onboard │ │ status  │ │  agent  │ │ gateway  │ │channels │ │
│  │  (init) │ │ (check) │ │  (CLI)  │ │ (daemon) │ │ status  │ │
│  └─────────┘ └─────────┘ └────┬────┘ └────┬─────┘ └─────────┘ │
└────────────────────────────────┼───────────┼─────────────────────┘
                                 │           │
─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┼ ─ ─ ─ ─ ─┼ ─ ─ ─ ─ ─ ─ ─ ─ ─
                                 │           │
┌────────────────────────────────┼───────────┼─────────────────────┐
│                        nanobot-core (Library)                    │
│                                │           │                     │
│  ┌─────────────────────────────▼───────────▼──────────────────┐  │
│  │                      Agent Loop (核心引擎)                  │  │
│  │  ┌────────────┐  ┌──────────────┐  ┌──────────────────┐   │  │
│  │  │  Context   │  │    Tool      │  │    History        │   │  │
│  │  │  Builder   │  │   Executor   │  │   Processor       │   │  │
│  │  └────────────┘  └──────────────┘  └──────────────────┘   │  │
│  └──────────┬──────────────┬──────────────────┬──────────────┘  │
│             │              │                  │                  │
│  ┌──────────▼──────┐  ┌───▼──────────┐  ┌───▼──────────────┐  │
│  │  Providers      │  │  Tool        │  │   Session        │  │
│  │  (LLM 抽象层)   │  │  Registry    │  │   Manager        │  │
│  │                 │  │              │  │                   │  │
│  │ ┌─────────────┐│  │ ┌──────────┐ │  │  ┌─────────────┐ │  │
│  │ │  OpenAI     ││  │ │Filesystem│ │  │  │  Sessions    │ │  │
│  │ │  Compatible ││  │ │Shell     │ │  │  │  (HashMap)   │ │  │
│  │ │  Provider   ││  │ │WebSearch │ │  │  └──────┬──────┘ │  │
│  │ ├─────────────┤│  │ │WebFetch  │ │  │         │        │  │
│  │ │  Gemini     ││  │ │Spawn    │ │  └─────────┼────────┘  │
│  │ │  Provider   ││  │ │Message  │ │            │           │
│  │ └─────────────┘│  │ │Cron     │ │            │           │
│  └────────────────┘  │ │MCP Tools│ │  ┌─────────▼────────┐  │
│                      │ └──────────┘ │  │  Memory Store    │  │
│  ┌────────────────┐  └──────────────┘  │  (SQLite)        │  │
│  │  Message Bus   │                    │  ┌─────────────┐ │  │
│  │  (mpsc 通道)   │◄───────────────────┤  │ memories    │ │  │
│  │                │                    │  │ sessions    │ │  │
│  │  inbound_tx/rx │                    │  │ session_msg │ │  │
│  │  outbound_tx/rx│                    │  │ history     │ │  │
│  └───────┬────────┘                    │  │ kv_store    │ │  │
│          │                             │  │ tasks       │ │  │
│  ┌───────▼──────────────────────────┐  │  └─────────────┘ │  │
│  │        Channel Manager           │  └──────────────────┘  │
│  │  ┌──────┐ ┌───────┐ ┌────────┐  │                        │
│  │  │Tele- │ │Discord│ │ Slack  │  │  ┌───────────────────┐  │
│  │  │gram  │ │       │ │        │  │  │   Config Loader   │  │
│  │  ├──────┤ ├───────┤ ├────────┤  │  │   (YAML → Struct) │  │
│  │  │飞书  │ │ 邮件  │ │ 钉钉  │  │  └───────────────────┘  │
│  │  ├──────┤ ├───────┤ ├────────┤  │                        │
│  │  │企业  │ │Webhook│ │  CLI   │  │  ┌───────────────────┐  │
│  │  │微信  │ │Server │ │       │  │  │   Skills Loader   │  │
│  │  └──────┘ └───────┘ └────────┘  │  │   (MD → Context)  │  │
│  └─────────────────────────────────┘  └───────────────────┘  │
│                                                               │
│  ┌───────────────┐  ┌────────────────┐  ┌──────────────────┐ │
│  │  Heartbeat    │  │  Cron Service  │  │  MCP Client      │ │
│  │  Service      │  │  (定时任务)     │  │  (JSON-RPC 2.0)  │ │
│  └───────────────┘  └────────────────┘  └──────────────────┘ │
└──────────────────────────────────────────────────────────────────┘

                    ┌─────────────────────┐
                    │   External LLM APIs  │
                    │  OpenAI / Anthropic  │
                    │  DeepSeek / 智谱     │
                    │  Ollama / Gemini     │
                    └─────────────────────┘
```

---

## 3. 数据流动图

### 3.1 CLI 模式数据流

```
用户输入
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  reedline    │───▶│  AgentLoop   │───▶│  Context     │
│  (REPL)      │    │  .process_   │    │  Builder     │
│              │    │   direct()   │    │              │
└──────────────┘    └──────┬───────┘    └──────┬───────┘
                           │                    │
                    ┌──────▼───────┐     ┌──────▼───────┐
                    │   Session    │     │ 构建 System  │
                    │   Manager   │     │ Prompt +     │
                    │  ┌────────┐ │     │ History +    │
                    │  │save    │ │     │ User Msg     │
                    │  │user msg│ │     └──────┬───────┘
                    │  └────────┘ │            │
                    └─────────────┘     ┌──────▼───────┐
                                        │  ChatRequest │
                                        │  (messages,  │
                                        │   tools,     │
                                        │   model)     │
                                        └──────┬───────┘
                                               │
                           ┌───────────────────▼─────────────────────┐
                           │          LLM Provider (chat/stream)     │
                           │    ┌──────┐  ┌──────┐  ┌──────────────┐│
                           │    │OpenAI│  │DeepSk│  │ OpenRouter   ││
                           │    │ API  │  │ API  │  │    API       ││
                           │    └──────┘  └──────┘  └──────────────┘│
                           └───────────────────┬─────────────────────┘
                                               │
                                        ┌──────▼───────┐
                                        │ ChatResponse │
                                        │ ┌──────────┐ │
                                        │ │ content  │ │
                                        │ │ tool_    │ │
                                        │ │  calls   │ │
                                        │ │ reasoning│ │
                                        │ └──────────┘ │
                                        └──────┬───────┘
                                               │
                              ┌────────────────┼────────────────┐
                              │ has_tool_calls?│                │
                              │                │                │
                        ┌─────▼─────┐    ┌─────▼──────┐        │
                        │  YES      │    │   NO       │        │
                        │           │    │            │        │
                  ┌─────▼──────┐   │    │  最终响应   │        │
                  │  Tool      │   │    │  返回用户   │        │
                  │  Executor  │   │    └────────────┘        │
                  │            │   │                           │
                  │ execute_   │   │                           │
                  │  batch()   │   │                           │
                  └─────┬──────┘   │                           │
                        │          │                           │
                  ┌─────▼──────┐   │                           │
                  │ Tool Result│   │                           │
                  │ append to  │   │                           │
                  │ messages   │───┘ (循环回到 LLM Provider)
                  └────────────┘
```

### 3.2 Gateway 模式数据流

```
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ Telegram │  │ Discord  │  │  Slack   │  │  飞书    │  │  Email   │
│   Bot    │  │   Bot    │  │  WSS     │  │ Webhook  │  │  IMAP    │
└────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘
     │             │             │             │             │
     └──────┬──────┴──────┬──────┴──────┬──────┘             │
            │             │             │                     │
     ┌──────▼─────────────▼─────────────▼─────────────────────▼───┐
     │                    InboundMessage                           │
     │  { channel, sender_id, chat_id, content, media, metadata } │
     └───────────────────────────┬────────────────────────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │     Middleware Layer     │
                    │  ┌──────┐  ┌─────────┐  │
                    │  │Auth  │  │Rate     │  │
                    │  │Check │  │Limiter  │  │
                    │  └──────┘  └─────────┘  │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │   MessageBus.inbound_tx  │
                    │  (mpsc::Sender, cap=100) │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │   Inbound Handler Task  │
                    │  (tokio::spawn per msg) │
                    │                         │
                    │  msg ──▶ AgentLoop      │
                    │         .process_direct │
                    │         (session_key)   │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │     AgentResponse       │
                    │  { content, reasoning,  │
                    │    tools_used }         │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │  MessageBus.outbound_tx  │
                    │  OutboundMessage {       │
                    │    channel, chat_id,     │
                    │    content }             │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │   Outbound Router Task  │
                    │   ChannelManager.route() │
                    └────┬──────┬──────┬──────┘
                         │      │      │
               ┌─────────▼┐ ┌──▼────┐ ┌▼────────┐
               │ Telegram  │ │Slack  │ │ 飞书    │  ...
               │  .send()  │ │.send()│ │ .send() │
               └───────────┘ └───────┘ └─────────┘
```

### 3.3 Heartbeat & Cron 数据流

```
┌─────────────────────────┐    ┌──────────────────────────┐
│  HeartbeatService       │    │  CronService              │
│                         │    │                            │
│  读取 HEARTBEAT.md      │    │  每 60 秒检查 SQLite      │
│  解析 cron 表达式       │    │  中的 cron_jobs 表         │
│  到达触发时间 →          │    │  到期任务 →                │
└───────────┬─────────────┘    └────────────┬──────────────┘
            │                                │
            ▼                                ▼
   InboundMessage                   InboundMessage
   sender_id: "heartbeat"          sender_id: "cron"
   content: task_text              content: job.message
            │                                │
            └──────────┬─────────────────────┘
                       │
              ┌────────▼─────────┐
              │  MessageBus      │
              │  .inbound_tx     │
              └────────┬─────────┘
                       │
              ┌────────▼─────────┐
              │  Agent 正常处理   │
              │  (与普通消息相同) │
              └──────────────────┘
```

---

## 4. Agent 执行流程图

```
                              ┌──────────────┐
                              │   开始处理    │
                              │  process_    │
                              │  direct()    │
                              └──────┬───────┘
                                     │
                              ┌──────▼───────┐
                              │ 处理斜杠命令  │
                              │ /new → 清空   │
                              │ /help → 帮助  │
                              └──────┬───────┘
                                     │ (非斜杠命令)
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  1. 保存 user message 到 Session       │
                 │  2. 追加到 history (SQLite)             │
                 │  3. 获取历史快照 (memory_window 条)     │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  ContextBuilder.build_messages()       │
                 │                                        │
                 │  ┌──────────────────────────────────┐  │
                 │  │ [system] SOUL.md + AGENTS.md +   │  │
                 │  │          USER.md + TOOLS.md +    │  │
                 │  │          skills_context +         │  │
                 │  │          long_term_memory         │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [历史消息 × N]                    │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] 当前输入内容               │  │
                 │  └──────────────────────────────────┘  │
                 └───────────────────┬───────────────────┘
                                     │
                              ┌──────▼───────┐
                              │ iteration = 0│
                              └──────┬───────┘
                                     │
                  ┌──────────────────▼──────────────────┐
            ┌─────│ iteration < max_iterations (默认 20)?│
            │     └──────────────────┬──────────────────┘
            │ NO                     │ YES
            │                 ┌──────▼───────┐
            │                 │ iteration++  │
            │                 └──────┬───────┘
            │                        │
            │                 ┌──────▼───────────────────┐
            │                 │ 构建 ChatRequest:         │
            │                 │  model, messages, tools,  │
            │                 │  temperature, max_tokens,  │
            │                 │  thinking                  │
            │                 └──────┬───────────────────┘
            │                        │
            │                 ┌──────▼───────────────────┐
            │                 │ LLM Provider.chat() /     │
            │                 │         .chat_stream()    │
            │                 │                           │
            │                 │ 失败 → 指数退避重试 ×3    │
            │                 └──────┬───────────────────┘
            │                        │
            │                 ┌──────▼───────┐
            │                 │ ChatResponse  │
            │                 └──────┬───────┘
            │                        │
            │              ┌─────────▼─────────┐
            │              │ has_tool_calls()?  │
            │              └────┬──────────┬───┘
            │                   │ YES      │ NO
            │            ┌──────▼──────┐   │
            │            │ ToolExecutor │   │
            │            │.execute_    │   │
            │            │ batch()     │   │
            │            │             │   │
            │            │ 并行执行所有 │   │
            │            │ tool_calls  │   │
            │            └──────┬──────┘   │
            │                   │          │
            │            ┌──────▼──────┐   │
            │            │ 将 tool     │   │
            │            │ results    │   │
            │            │ 追加到     │   │
            │            │ messages   │   │
            │            └──────┬──────┘   │
            │                   │          │
            │                   ▼          │
            │           (回到循环顶部)      │
            │                              │
            │                       ┌──────▼──────┐
            └──────────────────────▶│ 返回最终响应 │
                                    │ AgentResponse│
                                    │ {content,    │
                                    │  reasoning,  │
                                    │  tools_used} │
                                    └──────┬──────┘
                                           │
                                    ┌──────▼──────┐
                                    │ 保存 assistant│
                                    │ message 到   │
                                    │ Session +    │
                                    │ History      │
                                    └─────────────┘
```

### 4.1 流式输出流程

```
chat_stream() ──▶ Stream<ChatStreamChunk>
                        │
                        ▼
               accumulate_stream()
                        │
           ┌────────────┼────────────┐
           │            │            │
    delta.content  delta.reasoning  delta.tool_calls
           │            │            │
           ▼            ▼            ▼
    StreamEvent::   StreamEvent::   tool_calls_map
    Content(text)   Reasoning(text) (累积直到流结束)
           │            │            │
           ▼            ▼            ▼
    callback()      callback()    解析为 Vec<ToolCall>
    (实时输出)      (实时输出)    → ChatResponse
```

---

## 5. 存储架构图

### 5.1 SQLite 数据库结构

```
~/.nanobot/nanobot.db  (SqliteStore)
│
├── sessions              会话元数据
│   ├── key TEXT PK       会话标识 (如 "cli:interactive", "telegram:12345")
│   └── last_consolidated INTEGER
│
├── session_messages      每条消息独立存储
│   ├── id INTEGER PK
│   ├── session_key TEXT  → sessions.key
│   ├── role TEXT         "user" | "assistant"
│   ├── content TEXT      消息内容
│   ├── timestamp TEXT    ISO 8601
│   └── tools_used TEXT   JSON 数组 (nullable)
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
├── history               追加式历史日志
│   ├── id INTEGER PK
│   ├── content TEXT      "User: ..." / "Assistant: ..."
│   └── timestamp TEXT
│
├── kv_store              键值对 (长期记忆)
│   ├── key TEXT PK       如 "MEMORY"
│   └── value TEXT        MEMORY.md 内容
│
├── tasks                 子代理任务
│   ├── id TEXT PK
│   ├── prompt TEXT
│   ├── channel TEXT
│   ├── chat_id TEXT
│   ├── session_key TEXT
│   ├── status TEXT       "pending"|"running"|"completed"|"failed"
│   ├── priority TEXT     "low"|"normal"|"high"|"urgent"
│   ├── created_at TEXT
│   ├── started_at TEXT
│   ├── completed_at TEXT
│   ├── result TEXT
│   ├── error TEXT
│   ├── timeout_secs INTEGER
│   ├── progress INTEGER
│   └── metadata TEXT     JSON
│
└── cron_jobs             定时任务
    ├── id TEXT PK
    ├── name TEXT
    ├── cron_expr TEXT    cron 表达式
    ├── message TEXT      触发时发送的消息
    ├── channel TEXT
    ├── chat_id TEXT
    ├── last_run TEXT
    └── next_run TEXT
```

### 5.2 存储写入流程

```
用户发送消息
       │
       ├──▶ SessionManager.append_message("user", content)
       │         │
       │         ├──▶ Session.messages.push(SessionMessage)
       │         └──▶ SqliteStore: INSERT INTO session_messages
       │
       └──▶ MemoryStore.append_history("User: ...")
                 │
                 └──▶ SqliteStore: INSERT INTO history

Agent 回复
       │
       ├──▶ SessionManager.append_message("assistant", response)
       │         │
       │         ├──▶ Session.messages.push(SessionMessage)
       │         └──▶ SqliteStore: INSERT INTO session_messages
       │
       └──▶ MemoryStore.append_history("Assistant: ...")
                 │
                 └──▶ SqliteStore: INSERT INTO history
```

### 5.3 文件系统存储

```
~/.nanobot/                 工作空间根目录
├── config.yaml             主配置文件
├── nanobot.db              SQLite 数据库
├── AGENTS.md               Agent 人格/行为描述
├── SOUL.md                 Agent 灵魂/价值观定义
├── USER.md                 用户信息描述
├── TOOLS.md                可用工具说明
├── HEARTBEAT.md            心跳定时任务配置
├── memory/                 记忆目录
│   └── MEMORY.md           长期记忆 (kv_store 备份)
└── skills/                 用户自定义技能
    └── *.md                Markdown + YAML frontmatter
```

---

## 6. 模块详解

### 6.1 providers/ — LLM 提供商抽象层

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
│OpenAI         │ │  Gemini     │ │  (可扩展)    │
│Compatible     │ │  Provider   │ │              │
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
│ └───────────┘ │
└───────────────┘
```

**ModelSpec** 解析格式: `provider_id/model_id` 或 `model_id`

| 输入 | provider | model |
|------|----------|-------|
| `deepseek/deepseek-chat` | `deepseek` | `deepseek-chat` |
| `anthropic/claude-4.5-sonnet` | `anthropic` | `claude-4.5-sonnet` |
| `gpt-4o` | `None` (使用默认) | `gpt-4o` |

### 6.2 tools/ — 工具系统

```
trait Tool: Send + Sync
├── name() → &str
├── description() → &str
├── parameters() → serde_json::Value  (JSON Schema)
└── execute(args: Value) → ToolResult
```

| 工具 | 类名 | 类别 | 需审批 | 说明 |
|------|------|------|--------|------|
| `read_file` | ReadFileTool | filesystem | No | 读取文件内容 |
| `write_file` | WriteFileTool | filesystem | Yes | 写入文件 |
| `edit_file` | EditFileTool | filesystem | Yes | 编辑文件 (search/replace) |
| `list_dir` | ListDirTool | filesystem | No | 列出目录内容 |
| `exec` | ExecTool | system | Yes | 执行 Shell 命令 (带超时) |
| `spawn` | SpawnTool | system | No | 创建子代理执行任务 |
| `web_fetch` | WebFetchTool | web | No | HTTP GET 请求 |
| `web_search` | WebSearchTool | web | No | Web 搜索 (Brave/Tavily/Exa) |
| `message` | MessageTool | communication | No | 通过 Bus 发消息到渠道 |
| `cron` | CronTool | system | No | 管理定时任务 |
| MCP tools | (动态) | mcp | (动态) | MCP 服务器提供的工具 |

### 6.3 channels/ — 通信渠道

```
trait Channel: Send + Sync
├── name() → &str
├── start() → Result<()>
├── stop() → Result<()>
├── send(OutboundMessage) → Result<()>
└── graceful_shutdown() → Result<()>
```

| 渠道 | Feature Flag | 传输协议 | 说明 |
|------|-------------|----------|------|
| Telegram | `telegram` | Long Polling (teloxide) | Telegram Bot API |
| Discord | `discord` | WebSocket (serenity) | Discord Gateway |
| Slack | `slack` | WebSocket (tungstenite) | Slack Socket Mode |
| 飞书 | `feishu` | HTTP Webhook (axum) | 飞书事件订阅 |
| 邮件 | `email` | IMAP Polling + SMTP | 邮件收发 |
| 钉钉 | `dingtalk` | HTTP Webhook (axum) | 钉钉回调 |
| 企业微信 | `wecom` | HTTP Webhook (axum) | 企微回调 |

### 6.4 mcp/ — Model Context Protocol

```
┌─────────────┐    JSON-RPC 2.0     ┌──────────────────┐
│  MCP Client │◄───── stdio ───────▶│  MCP Server      │
│  (nanobot)  │                     │  (外部进程)       │
│             │                     │                   │
│  initialize │────────────────────▶│  返回 tool 列表   │
│  tools/list │────────────────────▶│  返回 tool 定义   │
│  tools/call │────────────────────▶│  执行并返回结果   │
└─────────────┘                     └──────────────────┘

配置示例:
  tools:
    mcp_servers:
      my-server:
        command: "npx"
        args: ["-y", "my-mcp-server"]
```

---

## 7. 关键数据结构

### 7.1 消息流转结构

```rust
// 入站消息 (外部 → Agent)
InboundMessage {
    channel: ChannelType,     // Telegram | Discord | Cli | ...
    sender_id: String,        // 发送者 ID
    chat_id: String,          // 对话 ID
    content: String,          // 消息正文
    media: Option<Vec<MediaAttachment>>,
    metadata: Option<Value>,
    timestamp: DateTime<Utc>,
    trace_id: Option<String>,
}

// 出站消息 (Agent → 外部)
OutboundMessage {
    channel: ChannelType,
    chat_id: String,
    content: String,
    metadata: Option<Value>,
    trace_id: Option<String>,
}

// Session Key 生成规则:
//   "{channel_type}:{chat_id}"
//   例: "telegram:12345", "cli:interactive"
```

### 7.2 LLM 请求/响应结构

```rust
ChatRequest {
    model: String,                        // "deepseek-chat"
    messages: Vec<ChatMessage>,           // 对话历史
    tools: Option<Vec<ToolDefinition>>,   // 可用工具
    temperature: Option<f32>,             // 0.0 ~ 2.0
    max_tokens: Option<u32>,              // 最大生成 token
    thinking: Option<ThinkingConfig>,     // 推理模式
}

ChatMessage {
    role: String,                         // "system"|"user"|"assistant"|"tool"
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,    // assistant 发起的工具调用
    tool_call_id: Option<String>,         // tool result 的对应 ID
    name: Option<String>,                 // tool name
}

ChatResponse {
    content: Option<String>,              // 文本回复
    tool_calls: Vec<ToolCall>,            // 工具调用请求
    reasoning_content: Option<String>,    // 推理/思考内容
}
```

### 7.3 历史处理策略

| 策略 | 说明 |
|------|------|
| `DirectInjectStrategy` | 直接注入所有历史, 不做处理 |
| `TruncateStrategy` | 保留最近 N 条, 截断早期消息 |
| `TokenBudgetStrategy` | 按 token 预算分配, 优先保留近期 |
| `SummarizeStrategy` | 超过阈值时自动摘要早期历史 |
| `RelevanceFilterStrategy` | 按相关性过滤, 保留与当前输入相关的 |
| `CombinedStrategy` | 组合多个策略 |

---

## 8. 安装与使用教程

### 8.1 前置要求

- Rust 1.75+ (推荐使用 `rustup` 安装)
- SQLite3 (bundled, 无需单独安装)
- 至少一个 LLM API Key

### 8.2 构建安装

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

### 8.3 初始化

```bash
# 初始化配置和工作空间
nanobot onboard
```

这会创建:
- `~/.nanobot/config.yaml` — 主配置文件
- `~/.nanobot/AGENTS.md` — Agent 行为定义
- `~/.nanobot/SOUL.md` — Agent 灵魂
- `~/.nanobot/USER.md` — 用户信息
- `~/.nanobot/TOOLS.md` — 工具说明
- `~/.nanobot/HEARTBEAT.md` — 心跳任务
- `~/.nanobot/memory/` — 记忆目录
- `~/.nanobot/skills/` — 自定义技能目录

### 8.4 配置文件

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

### 8.5 命令行使用

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

### 8.6 交互模式内置命令

| 命令 | 说明 |
|------|------|
| `/new` | 清空当前会话, 开始新对话 |
| `/help` | 显示可用命令 |
| `/exit`, `/quit`, `exit`, `quit`, `:q` | 退出交互模式 |
| `Ctrl+C`, `Ctrl+D` | 退出 |

### 8.7 自定义技能

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

### 8.8 环境变量

| 变量 | 说明 |
|------|------|
| `RUST_LOG` | 日志级别 (`debug`, `info`, `warn`, `error`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry 端点 (如 `http://localhost:4317`) |
| `OTEL_SDK_DISABLED=true` | 禁用 OpenTelemetry |

### 8.9 典型使用场景

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

## 附录: 技术栈一览

| 领域 | 技术 |
|------|------|
| 语言 | Rust 2021 Edition |
| 异步运行时 | tokio (full) |
| HTTP 客户端 | reqwest + rustls-tls |
| HTTP 服务器 | axum + tower |
| CLI 框架 | clap 4 (derive) |
| REPL | reedline |
| 序列化 | serde + serde_json + serde_yaml |
| 数据库 | rusqlite (bundled SQLite) |
| Telegram | teloxide |
| Discord | serenity |
| Slack | tokio-tungstenite (WebSocket) |
| Email | lettre (SMTP) + async-imap (IMAP) |
| 日志/追踪 | tracing + tracing-subscriber |
| 分布式追踪 | OpenTelemetry + OTLP |
| Markdown 渲染 | termimad |
| 终端颜色 | colored |
| Cron 解析 | cron 0.15 |
