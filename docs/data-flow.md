# 数据流设计

> Nanobot-RS 各模式下的数据流转路径

---

## 1. CLI 模式数据流

```
用户输入
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  reedline    │───▶│  AgentLoop   │───▶│   Prompt     │
│  (REPL)      │    │  .process_   │    │   Loader     │
│              │    │   direct()   │    │              │
└──────────────┘    └──────┬───────┘    └──────┬───────┘
                           │                    │
                    ┌──────▼───────┐     ┌──────▼───────┐
                    │   Session    │     │ 构建 System  │
                    │   Manager   │     │ Prompt:      │
                    │  (SQLite)   │     │ PROFILE.md + │
                    │  ┌────────┐ │     │ SOUL.md +    │
                    │  │save    │ │     │ AGENTS.md +  │
                    │  │user msg│ │     │ MEMORY.md +  │
                    │  └────────┘ │     │ BOOTSTRAP.md │
                    └─────────────┘     │ + skills     │
                                        └──────┬───────┘
                                               │
                                        ┌──────▼───────┐
                                        │ ChatRequest  │
                                        │ (messages,   │
                                        │  tools,      │
                                        │  model)      │
                                        └──────┬───────┘
                                               │
                           ┌───────────────────▼─────────────────────┐
                           │          LLM Provider (chat/stream)     │
                           │    ┌──────┐  ┌──────┐  ┌──────────────┐│
                           │    │OpenAI│  │Gemini│  │   Copilot    ││
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
                  │ (并行执行)  │   │                           │
                  └─────┬──────┘   │                           │
                        │          │                           │
                  ┌─────▼──────┐   │                           │
                  │ Tool Result│   │                           │
                  │ append to  │   │                           │
                  │ messages   │───┘ (循环回到 LLM Provider)
                  └────────────┘
```

---

## 2. Gateway 模式数据流 (Actor 模型)

```
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ Telegram │  │ Discord  │  │  Slack   │  │  飞书    │  │ WebSocket│
│   Bot    │  │   Bot    │  │  WSS     │  │ Webhook  │  │  Server  │
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
                    │      Router Actor       │
                    │  (单任务,拥有路由表)      │
                    │                         │
                    │  HashMap<SessionKey,    │
                    │    mpsc::Sender>        │
                    │  • 按 session_key 分发  │
                    │  • 懒创建 Session Actor  │
                    │  • 清理已关闭的 channel  │
                    └────┬──────┬──────┬──────┘
                         │      │      │
              ┌──────────▼┐ ┌──▼────┐ ┌▼──────────┐
              │ Session   │ │Session│ │ Session   │
              │ Actor #1  │ │Act #2 │ │ Actor #N  │
              │           │ │       │ │           │
              │ 串行处理   │ ...     │ ...         │
              │ AgentLoop │ │       │ │           │
              │ .process_ │ │       │ │           │
              │  direct() │ │       │ │           │
              │           │ │       │ │           │
              │ 空闲超时   │ │       │ │           │
              │ 自动销毁   │ │       │ │           │
              └──────┬────┘ └──┬────┘ └─────┬─────┘
                     │         │            │
                     └────┬────┘────────────┘
                          │
              ┌───────────▼───────────┐
              │    Outbound Actor     │
              │  (单任务,专职发送)     │
              │                       │
              │  send_outbound()      │
              │  按 channel 类型路由   │
              └───┬──────┬──────┬────┘
                  │      │      │
        ┌─────────▼┐ ┌──▼────┐ ┌▼────────┐
        │ Telegram  │ │Slack  │ │WebSocket│  ...
        │  .send()  │ │.send()│ │ .send() │
        └───────────┘ └───────┘ └─────────┘
```

### Actor 模型设计要点

| Actor | 职责 | 并发模型 |
|-------|------|----------|
| **Router Actor** | 按 SessionKey 分发消息到 Session Actor，懒创建/清理 | 单任务，拥有路由表 HashMap，零锁 |
| **Session Actor** | 串行处理单个 session 的所有消息，调用 AgentLoop | 每 session 独立 tokio::spawn，共享 `Arc<AgentLoop>` |
| **Outbound Actor** | 跨网络 HTTP/WebSocket 发送，不阻塞上游 | 单任务，即使外部 API 阻塞也不影响 Agent |

---

## 3. Heartbeat & Cron 数据流

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
              │  Router Actor    │
              │  (Gateway 模式)   │
              │  或 AgentLoop    │
              │  .process_direct │
              │  (CLI 模式)      │
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
                              │ pre_request  │
                              │ Hook (可选)  │
                              │ 可修改/中止  │
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
                 │  2. 获取历史快照 (memory_window 条)     │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  History Processor (token 感知)        │
                 │                                        │
                 │  算法:                                  │
                 │  1. 取最近 max_messages 条              │
                 │  2. 始终保留最后 recent_keep 条          │
                 │  3. 较早消息按 token 预算纳入/驱逐       │
                 │  → ProcessedHistory {                   │
                 │      messages: 保留的消息,               │
                 │      evicted: 被驱逐的消息               │
                 │    }                                    │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  ContextCompressionHook.compress()     │
                 │                                        │
                 │  evicted 不为空 → LLM 摘要生成          │
                 │  evicted 为空 → 加载已有摘要             │
                 │  → summary: Option<String>              │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  Prompt Assembly                       │
                 │                                        │
                 │  ┌──────────────────────────────────┐  │
                 │  │ [system] PROFILE.md + SOUL.md +  │  │
                 │  │          AGENTS.md + MEMORY.md + │  │
                 │  │          BOOTSTRAP.md +           │  │
                 │  │          skills_context             │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [assistant] 摘要 (如有)           │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [历史消息 × N] (已处理)           │  │
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
                                    ┌──────▼───────┐
                                    │ post_response│
                                    │ Hook (可选)  │
                                    │ 审计/告警    │
                                    └──────┬───────┘
                                           │
                                    ┌──────▼──────┐
                                    │ 保存 assistant│
                                    │ message 到   │
                                    │ Session      │
                                    └─────────────┘
```

---

## 5. 流式输出流程

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

## 6. Vault 注入流程

```
用户消息: "使用 {{vault:api_key}} 调用 API"
                    │
                    ▼
          ┌─────────────────┐
          │  VaultInjector  │
          │  .inject()      │
          └────────┬────────┘
                   │
         ┌─────────▼─────────┐
         │  scan_placeholders│
         │  提取 {{vault:*}} │
         └─────────┬─────────┘
                   │
         ┌─────────▼─────────┐
         │   VaultStore      │
         │   .get(key)       │
         │   (可能解密)      │
         └─────────┬─────────┘
                   │
         ┌─────────▼─────────┐
         │ replace_placeholders│
         │ 替换为实际值       │
         └─────────┬─────────┘
                   │
                   ▼
处理后的消息: "使用 sk-xxxx 调用 API"
                   │
                   ▼
            AgentLoop 处理
```

### InjectionReport

```rust
InjectionReport {
    total_placeholders: 1,
    replaced: 1,
    missing_keys: [],      // 未找到的密钥会记录在此
}
```

---

## 7. SubagentManager 调度模式

```
─── submit() (异步 fire-and-forget) ───

调用者 ──▶ tokio::spawn ──▶ AgentLoop.process_direct() ──▶ OutboundMessage
  │                              │                              │
  │  立即返回 Ok(())             │  10 分钟超时                  │  通过 outbound_tx
  │                              │                              │  发送到渠道
  ▼                              ▼                              ▼
(不等待)                    (后台运行)                     (结果路由到 chat)


─── submit_and_wait() (同步等待) ───

调用者 ──▶ tokio::spawn ──▶ AgentLoop.process_direct() ──▶ oneshot::Sender
  │              │                                              │
  │  await rx    │  10 分钟超时                                  │ tx.send(result)
  │  (阻塞等待)  │                                              │
  ▼              ▼                                              ▼
(收到 AgentResponse                                    (oneshot channel)
 或 Error)
```
