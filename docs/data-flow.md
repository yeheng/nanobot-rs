# 数据流设计

> Gasket-RS 各模式下的数据流转路径

---

## 1. CLI 模式数据流

```
用户输入
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  reedline    │───▶│ AgentSession │───▶│   Prompt     │
│  (REPL)      │    │  .process_   │    │   Loader     │
│              │    │   direct()   │    │              │
└──────────────┘    └──────┬───────┘    └──────┬───────┘
                           │                    │
                    ┌──────▼───────┐     ┌──────▼───────┐
                    │    Agent     │     │ 构建 System  │
                    │   Context    │     │ Prompt:      │
                    │  (SQLite)    │     │ PROFILE.md + │
                    │  ┌────────┐  │     │ SOUL.md +    │
                    │  │save    │  │     │ AGENTS.md +  │
                    │  │user msg│  │     │ MEMORY.md +  │
                    │  └────────┘  │     │ BOOTSTRAP.md │
                    └──────────────┘     │ + skills     │
                                         └──────┬───────┘
                                                │
                                         ┌──────▼───────┐
                                         │ ChatRequest  │
                                         │ (messages,   │
                                         │  tools,      │
                                         │  model)      │
                                         └──────┬───────┘
                                                │
                           ┌────────────────────▼─────────────────────┐
                           │          LLM Provider (chat/stream)      │
                           │    ┌──────┐  ┌──────┐  ┌──────────────┐│
                           │    │OpenAI│  │Gemini│  │   Copilot    ││
                           │    │ API  │  │ API  │  │    API       ││
                           │    └──────┘  └──────┘  └──────────────┘│
                           └────────────────────┬────────────────────┘
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
                              ┌─────────────────┼─────────────────┐
                              │ has_tool_calls? │                 │
                              │                 │                 │
                        ┌─────▼─────┐    ┌─────▼──────┐         │
                        │  YES      │    │   NO       │         │
                        │           │    │            │         │
                  ┌─────▼──────┐   │    │  最终响应   │         │
                  │  Tool      │   │    │  返回用户   │         │
                  │  Executor  │   │    └────────────┘         │
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
                    │  • 懒创建 Session Actor │
                    │  • 清理已关闭的 channel │
                    └────┬──────┬──────┬──────┘
                         │      │      │
              ┌──────────▼┐ ┌──▼────┐ ┌▼──────────┐
              │ Session   │ │Session│ │ Session   │
              │ Actor #1  │ │Act #2 │ │ Actor #N  │
              │           │ │       │ │           │
              │ 串行处理   │ ...     │ ...         │
              │AgentSession│ │       │ │           │
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
| **Session Actor** | 串行处理单个 session 的所有消息，调用 AgentSession | 每 session 独立 tokio::spawn，共享 `Arc<AgentSession>` |
| **Outbound Actor** | 跨网络 HTTP/WebSocket 发送，不阻塞上游 | 单任务，即使外部 API 阻塞也不影响 Agent |

### WebSocket 流式处理

```
Session Actor
    │
    ▼
AgentSession::process_direct_streaming_with_channel()
    │
    ▼
mpsc::Receiver<StreamEvent>
    │
    ▼
stream_event_to_ws_message()
    │
    ├──▶ StreamEvent::Content ──▶ WebSocketMessage::Text
    ├──▶ StreamEvent::Thinking ──▶ WebSocketMessage::Thinking
    ├──▶ StreamEvent::ToolStart ──▶ WebSocketMessage::ToolStart
    ├──▶ StreamEvent::ToolEnd ──▶ WebSocketMessage::ToolEnd
    └──▶ StreamEvent::Done ──▶ WebSocketMessage::Done
    │
    ▼
Outbound Actor ──▶ WebSocket 客户端
```

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
              │  或 AgentSession │
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
                              │ AgentSession │
                              │.process_direc│
                              │     t()      │
                              └──────┬───────┘
                                     │
                              ┌──────▼───────┐
                              │ BeforeRequest│
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
                 │  ContextCompactor::compact()           │
                 │                                        │
                 │  evicted 不为空 → 同步 LLM 摘要         │
                 │  evicted 为空 → 加载已有摘要            │
                 │  → summary: Option<String>             │
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
                 │  │ [system] 摘要 (如有)              │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] 历史消息 × N (已处理)      │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] 长期记忆 (动态加载)         │  │
                 │  │ [SYSTEM: Relevant memories...]    │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] 当前输入内容               │  │
                 │  └──────────────────────────────────┘  │
                 │                                        │
                 │  注：长期记忆作为 User Message 注入，   │
                 │  保护 System Prompt 的 Prompt Cache    │
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
            │              ┌─────────┴─────────┐
            │              │ has_tool_calls()?  │
            │              └────┬──────────┬───┘
            │                   │ YES      │ NO
            │            ┌──────▼──────┐   │
            │            │ ToolExecutor│   │
            │            │.execute_    │   │
            │            │ batch()     │   │
            │            │             │   │
            │            │ spawn_parallel│   │
            │            │ + 并行执行所有 │   │
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
                                    │ AfterResponse│
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

### 流式事件类型

```rust
pub enum StreamEvent {
    Thinking { agent_id: Option<Arc<str>>, content: Arc<str> },
    ToolStart { agent_id: Option<Arc<str>>, name: Arc<str>, arguments: Option<Arc<str>> },
    ToolEnd { agent_id: Option<Arc<str>>, name: Arc<str>, output: Option<Arc<str>> },
    Content { agent_id: Option<Arc<str>>, content: Arc<str> },
    Done { agent_id: Option<Arc<str>> },
    TokenStats { agent_id: Option<Arc<str>>, input_tokens: usize, output_tokens: usize, total_tokens: usize, cost: f64, currency: Arc<str> },
    SubagentStarted { agent_id: Arc<str>, task: Arc<str>, index: u32 },
    SubagentCompleted { agent_id: Arc<str>, index: u32, summary: Arc<str>, tool_count: u32 },
    SubagentError { agent_id: Arc<str>, index: u32, error: Arc<str> },
    Text { agent_id: Option<Arc<str>>, content: Arc<str> },
}
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
            AgentSession 处理
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

## 7. 子代理调度模式

### 7.1 纯函数创建（推荐）

```
调用者 ──▶ TaskSpec::new(id, prompt)
              │
              ├──▶ .with_model()
              ├──▶ .with_system_prompt()
              │
              ▼
    spawn_subagent(provider, tools, workspace, task, event_tx, result_tx, token_tracker)
              │
              ▼
    tokio::spawn(async {
        AgentSession::process_direct_streaming()
    })
              │
              ▼
    StreamEvent ──▶ mpsc::channel ──▶ SubagentTracker
```

### 7.2 Fire-and-Forget 模式

```
调用者 ──▶ spawn_subagent(task, result_tx, ...)
  │
  │  返回 JoinHandle
  │
  ▼
tokio::spawn ──▶ AgentSession::process_direct() ──▶ OutboundMessage
                     │                              │
                     │  10 分钟超时                  │  通过 outbound_tx
                     │                              │  发送到渠道
                     ▼                              ▼
               (后台运行)                     (结果路由到 chat)
```

### 7.3 同步等待模式

```
调用者 ──▶ spawn_subagent(task, result_tx, ...)
  │              │
  │  await rx    │  tokio::spawn
  │  (阻塞等待)  │  │
  ▼              ▼  │
(收到 SubagentResult │
  或 channel 关闭)   │
                    ▼
              result_tx.send(result)
                    │
                    ▼
              (返回结果给调用者)
```

---

## 8. 上下文压缩数据流

```
finalize_response()
    │
    ▼
process_history() ──▶ 识别被驱逐消息
    │
    ▼
ContextCompactor::try_compact(key, estimated_tokens)
    │
    ├──▶ token_budget 未超限? ──▶ 返回
    │
    ▼
异步执行 {
    │
    ▼
    LLM 生成摘要
    │
    ▼
    EventStore::save_summary()
    │
    ▼
    SQLite 存储 Summary 事件
}
```

### 压缩执行策略

- 非阻塞压缩在 `finalize_response` 中触发
- 压缩在后台执行，不阻塞响应
- 每次响应都会检查并执行压缩（如需要）

---

## 9. Hook 系统数据流

```
AgentSession::process_direct()
    │
    ├──▶ BeforeRequest Hook ──▶ 可修改/中止请求
    │
    ▼
Load Session / Save User Message
    │
    ├──▶ AfterHistory Hook ──▶ 可添加上下文
    │
    ▼
Process History
    │
    ├──▶ BeforeLLM Hook ──▶ Vault 注入等最后修改
    │
    ▼
LLM Provider
    │
    ├──▶ AfterToolCall Hook ──▶ 并行执行，只读审计
    │
    ▼
Return Response
    │
    ├──▶ AfterResponse Hook ──▶ 并行执行，只读审计
    │
    ▼
Save Assistant Message
```

### Hook 执行策略

| Hook Point | 策略 | 可修改 | 可中止 |
|------------|------|--------|--------|
| BeforeRequest | Sequential | ✓ | ✓ |
| AfterHistory | Sequential | ✓ | ✗ |
| BeforeLLM | Sequential | ✓ | ✗ |
| AfterToolCall | Parallel | ✗ | ✗ |
| AfterResponse | Parallel | ✗ | ✗ |
