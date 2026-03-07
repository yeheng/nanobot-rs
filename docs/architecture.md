# 结构设计

> Nanobot-RS 系统架构总览

---

## Crate 结构

```
nanobot-rs/                    (Cargo workspace)
├── nanobot-core/              核心库 — 所有业务逻辑
│   └── src/
│       ├── agent/             Agent 核心引擎 (loop, executor, prompt, history, stream, summarization, subagent)
│       ├── bus/               消息总线 (Actor 模型: Router/Session/Outbound)
│       ├── channels/          通信渠道 (Telegram, Discord, Slack, 飞书, 邮件, 钉钉, 企业微信, WebSocket)
│       ├── config/            配置加载 (YAML → Struct)
│       ├── cron/              定时任务服务
│       ├── crypto/            加密工具
│       ├── heartbeat/         心跳服务
│       ├── hooks/             外部 Shell Hook 系统
│       ├── mcp/               MCP 协议 (client, manager, tool, types)
│       ├── memory/            存储层 (MemoryStore trait + SQLite FTS5)
│       ├── pipeline/          多 Agent 协作管线 (三省六部, opt-in) → 详见 pipeline.md
│       ├── providers/         LLM 提供商 (OpenAI 兼容 + Gemini + Copilot)
│       ├── session/           会话管理 (SQLite 后端)
│       ├── skills/            技能加载器
│       ├── tools/             工具系统 (14 个内置 + 3 个管线工具)
│       ├── webhook/           Webhook 服务器
│       └── workspace/         工作空间模板文件
└── nanobot-cli/               CLI 可执行文件
    └── src/
        ├── main.rs            命令入口 + Gateway 启动器
        ├── cli.rs             CLI 交互模式
        ├── provider.rs        Provider 工厂
        └── commands/          子命令 (onboard, status, agent, gateway, channels)
```

---

## 系统架构图

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
│  │  │  Prompt    │  │    Tool      │  │    History        │   │  │
│  │  │  Loader    │  │   Executor   │  │   Processor      │   │  │
│  │  └────────────┘  └──────────────┘  └──────────────────┘   │  │
│  │  ┌────────────────────┐  ┌────────────────────────────┐   │  │
│  │  │  Summarization     │  │  Context Compression Hook  │   │  │
│  │  │  Service           │  │  (可扩展摘要策略)           │   │  │
│  │  └────────────────────┘  └────────────────────────────┘   │  │
│  └──────────┬──────────────┬──────────────────┬──────────────┘  │
│             │              │                  │                  │
│  ┌──────────▼──────┐  ┌───▼──────────┐  ┌───▼──────────────┐  │
│  │  Providers      │  │  Tool        │  │   Session        │  │
│  │  (LLM 抽象层)   │  │  Registry    │  │   Manager        │  │
│  │                 │  │              │  │   (SQLite 后端)   │  │
│  │ ┌─────────────┐│  │ ┌──────────┐ │  │                   │  │
│  │ │  OpenAI     ││  │ │Filesystem│ │  └─────────┬─────────┘  │
│  │ │  Compatible ││  │ │Shell     │ │            │            │
│  │ │  Provider   ││  │ │WebSearch │ │  ┌─────────▼─────────┐  │
│  │ ├─────────────┤│  │ │WebFetch  │ │  │  Memory Store     │  │
│  │ │  Gemini     ││  │ │Spawn    │ │  │  (SQLite + FTS5)  │  │
│  │ │  Provider   ││  │ │Message  │ │  │  ┌─────────────┐  │  │
│  │ ├─────────────┤│  │ │Cron     │ │  │  │ memories    │  │  │
│  │ │  Copilot    ││  │ │MCP Tools│ │  │  │ sessions    │  │  │
│  │ │  Provider   ││  │ │Memory   │ │  │  │ session_msg │  │  │
│  │ └─────────────┘│  │ │ Search  │ │  │  │ kv_store    │  │  │
│  └────────────────┘  │ │Sandbox  │ │  │  │ cron_jobs   │  │  │
│                      │ └──────────┘ │  │  │ summaries   │  │  │
│  ┌────────────────┐  └──────────────┘  │  └─────────────┘  │  │
│  │  Message Bus   │                    └───────────────────┘  │
│  │  (Actor 模型)  │                                           │
│  │                │                                           │
│  │  Router Actor  │   ┌───────────────────────────────────┐   │
│  │  Session Actor │   │   External Shell Hooks            │   │
│  │  Outbound Actor│   │   ~/.nanobot/hooks/               │   │
│  └───────┬────────┘   │   pre_request.sh                  │   │
│          │            │   post_response.sh                │   │
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
│  │              Pipeline (三省六部, opt-in)                 │  │
│  │                                                         │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │  │
│  │  │Orchestrator │  │  Permission  │  │  Stall        │  │  │
│  │  │  Actor      │  │   Matrix     │  │  Detector     │  │  │
│  │  │(mpsc event) │  │  (有向图)    │  │  (30s 扫描)   │  │  │
│  │  └──────┬──────┘  └──────────────┘  └───────────────┘  │  │
│  │         │                                               │  │
│  │  ┌──────▼────────────────────────────────────────────┐  │  │
│  │  │ TaskState 状态机 + PipelineStore (SQLite 3 表)    │  │  │
│  │  └───────────────────────────────────────────────────┘  │  │
│  │                                                         │  │
│  │  Tools: pipeline_task | delegate | report_progress      │  │
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
| **Option\<T\> 依赖注入** | 存储依赖使用 `Option<T>`：主 Agent 获得 `Some(impl)`，子 Agent 获得 `None`（无持久化） |
| **Actor 模型消息传递** | Gateway 使用三个 Actor（Router → Session → Outbound）通过 mpsc channel 通信，零锁设计 |
| **外部 Hook 扩展** | 遵循 UNIX 哲学，通过 `~/.nanobot/hooks/` 下的 Shell 脚本扩展，数据通过 stdin/stdout JSON 流转 |
| **Feature Flag 编译** | 各通信渠道通过 Cargo feature flag 独立编译，按需启用 |
| **无内存缓存** | SessionManager 直接读写 SQLite，利用 SQLite page cache 避免缓存一致性问题 |
| **Opt-in Pipeline** | 多 Agent 协作管线完全可选 — `pipeline.enabled: false` 时零开销，不创建表、不注册工具、不启动检测器 |
