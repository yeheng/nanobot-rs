# 数据流设计

> Gasket-RS 各模式下的数据流转路径

---

## 1. CLI 模式数据流

```mermaid
flowchart TB
    subgraph 用户输入
        U[用户输入]
    end

    subgraph REPL层
        R[reedline REPL]
    end

    subgraph AgentSession
        AS[AgentSession.process_direct]
    end

    subgraph 上下文组装
        PL[Prompt Loader]
        AC[Agent Context<br/>SQLite]
        SYS[构建 System Prompt<br/>PROFILE.md +<br/>SOUL.md +<br/>AGENTS.md +<br/>MEMORY.md +<br/>BOOTSTRAP.md + skills]
    end

    subgraph LLM调用
        CR[ChatRequest<br/>messages + tools + model]
        LLM[LLM Provider<br/>chat/stream]
    end

    U --> R --> AS

    AS --> AC
    AS --> PL
    PL --> SYS
    SYS --> CR

    CR --> LLM

    LLM --> CR2[ChatResponse<br/>content + tool_calls<br/>reasoning]

    CR2 --> TC{has_tool_calls?}

    TC -->|YES| TE[Tool Executor<br/>execute_batch<br/>并行执行]
    TE --> TR[Tool Result<br/>append to messages]
    TR --> LLM

    TC -->|NO| OUT[最终响应<br/>返回用户]

    style AS fill:#E3F2FD
    style LLM fill:#FFF3E0
    style TE fill:#F3E5F5
```

---

## 2. Gateway 模式数据流 (Actor 模型)

```mermaid
flowchart TB
    subgraph 用户入口
        TG[Telegram Bot]
        DC[Discord Bot]
        SL[Slack WSS]
        FS[飞书 Webhook]
        WS[WebSocket Server]
    end

    subgraph 消息处理
        IM[InboundMessage<br/>channel + sender_id<br/>chat_id + content<br/>media + metadata]
        MW[Middleware Layer<br/>Auth Check + Rate Limiter]
        RT[Router Actor<br/>HashMap SessionKey<br/>mpsc Sender<br/>懒创建/清理Session]
    end

    subgraph Session层
        S1[Session Actor #1]
        S2[Session Actor #2]
        SN[Session Actor #N]
    end

    subgraph Outbound层
        OB[Outbound Actor<br/>单任务专职发送<br/>send_outbound]
        OT[回复 Telegram]
        OS[回复 Slack]
        OW[回复 WebSocket]
    end

    TG --> IM
    DC --> IM
    SL --> IM
    FS --> IM
    WS --> IM

    IM --> MW --> RT

    RT --> S1
    RT --> S2
    RT --> SN

    S1 --> OB
    S2 --> OB
    SN --> OB

    OB --> OT
    OB --> OS
    OB --> OW

    style RT fill:#E3F2FD
    style OB fill:#FFF3E0
```

### Actor 模型设计要点

| Actor | 职责 | 并发模型 |
|-------|------|----------|
| **Router Actor** | 按 SessionKey 分发消息到 Session Actor，懒创建/清理 | 单任务，拥有路由表 HashMap，零锁 |
| **Session Actor** | 串行处理单个 session 的所有消息，调用 AgentSession | 每 session 独立 tokio::spawn，共享 `Arc<AgentSession>` |
| **Outbound Actor** | 跨网络 HTTP/WebSocket 发送，不阻塞上游 | 单任务，即使外部 API 阻塞也不影响 Agent |

### WebSocket 流式处理

```mermaid
flowchart TB
    SA[Session Actor]
    AS[AgentSession<br/>process_direct<br/>_streaming_with_channel]
    CH[mpsc Receiver<br/>StreamEvent]
    SW[stream_event<br/>_to_ws_message]

    subgraph 事件转换
        SE1[StreamEvent::Content]
        SE2[StreamEvent::Thinking]
        SE3[StreamEvent::ToolStart]
        SE4[StreamEvent::ToolEnd]
        SE5[StreamEvent::Done]
    end

    subgraph WebSocket消息
        WS1[WebSocketMessage<br/>Text]
        WS2[WebSocketMessage<br/>Thinking]
        WS3[WebSocketMessage<br/>ToolStart]
        WS4[WebSocketMessage<br/>ToolEnd]
        WS5[WebSocketMessage<br/>Done]
    end

    SA --> AS --> CH --> SW

    SW --> SE1 --> WS1
    SW --> SE2 --> WS2
    SW --> SE3 --> WS3
    SW --> SE4 --> WS4
    SW --> SE5 --> WS5

    WS1 --> OUT[Outbound Actor<br/>WebSocket 客户端]
    WS2 --> OUT
    WS3 --> OUT
    WS4 --> OUT
    WS5 --> OUT
```

---

## 3. Heartbeat & Cron 数据流

```mermaid
flowchart TB
    subgraph 定时任务
        HB[HeartbeatService<br/>读取 HEARTBEAT.md<br/>解析 cron 表达式<br/>到达触发时间]
        CS[CronService<br/>每60秒检查<br/>cron_jobs 表<br/>到期任务触发]
    end

    subgraph 消息生成
        IM1[InboundMessage<br/>sender_id: heartbeat<br/>content: task_text]
        IM2[InboundMessage<br/>sender_id: cron<br/>content: job.message]
    end

    IM1 --> HB
    IM2 --> CS

    HB --> RT[Router Actor<br/>Gateway 模式]
    CS --> RT

    RT --> AG[Agent 正常处理<br/>与普通消息相同]

    style HB fill:#FFE0B2
    style CS fill:#E3F2FD
```

---

## 4. Agent 执行流程图

```mermaid
flowchart TB
    START([开始]) --> PR[开始处理<br/>AgentSession<br/>process_direct]

    PR --> BR[BeforeRequest Hook<br/>可选<br/>可修改/中止]

    BR --> SL[处理斜杠命令<br/>/new → 清空<br/>/help → 帮助]

    SL --> SM{斜杠命令?}

    SM -->|YES| EX[执行命令]
    SM -->|NO| SH

    subgraph 保存消息
        SH[1. 保存 user message<br/>到 SessionEvent]
    end

    SH --> HH[History Processor<br/>token 感知]

    subgraph 历史处理
        HH --> HP[算法：<br/>1. 取最近 max_messages 条<br/>2. 始终保留最后 recent_keep 条<br/>3. 较早消息按 token 预算纳入/驱逐<br/>→ ProcessedHistory<br/>messages + evicted]
    end

    HP --> CC[ContextCompactor<br/>compact]

    CC --> EV{evicted<br/>不为空?}

    EV -->|YES| SUM[同步 LLM 摘要]
    EV -->|NO| LS[加载已有摘要]
    SUM --> SS[summary: Option String]
    LS --> SS

    SS --> PA[Prompt Assembly]

    subgraph Prompt组装
        PA --> SYS1["[system] PROFILE.md + SOUL.md +<br/>AGENTS.md + MEMORY.md +<br/>BOOTSTRAP.md + skills_context"]
        PA --> SYS2["[system] 摘要 (如有)"]
        PA --> USR1["[user] 历史消息 × N (已处理)"]
        PA --> USR2["[user] 长期记忆 (动态加载)<br/>Relevant memories..."]
        PA --> USR3["[user] 当前输入内容"]
    end

    PA --> I[iteration = 0]

    I --> LP{iteration &lt;<br/>max_iterations<br/>(默认20)?}

    LP -->|YES| INC[iteration++]
    INC --> CR[构建 ChatRequest<br/>model + messages + tools +<br/>temperature + max_tokens +<br/>thinking]

    CR --> LLM[LLM Provider<br/>chat / chat_stream]

    LLM --> LR{失败?}

    LR -->|YES| RET[指数退避重试 ×3]
    RET --> LLM

    LR -->|NO| CR2[ChatResponse]

    CR2 --> TC{has_tool<br/>_calls?}

    TC -->|YES| TE[ToolExecutor<br/>execute_batch<br/>并行执行]

    TE --> TR[Tool Result<br/>追加到 messages]

    TR --> I

    TC -->|NO| OUT[返回最终响应<br/>AgentResponse<br/>content + reasoning<br/>+ tools_used]

    LP -->|NO| OUT

    OUT --> AR[AfterResponse Hook<br/>可选<br/>审计/告警]

    AR --> SA[保存 assistant<br/>message 到<br/>Session]

    SA --> END([完成])

    style PR fill:#E3F2FD
    style LLM fill:#FFF3E0
    style TE fill:#F3E5F5
```

---

## 5. 流式输出流程

```mermaid
flowchart TB
    CS[chat_stream]
    AS[accumulate_stream]
    CH[StreamEvent]

    subgraph delta处理
        DC[delta.content]
        DR[delta.reasoning]
        DT[delta.tool_calls]
    end

    subgraph 事件转换
        EC[StreamEvent<br/>Content text]
        ER[StreamEvent<br/>Reasoning text]
        ET[tool_calls_map<br/>累积直到流结束]
    end

    CS --> AS --> DC
    AS --> DR
    AS --> DT

    DC --> EC
    DR --> ER
    DT --> ET

    EC --> CB[callback<br/>实时输出]
    ER --> CB
    ET -->|解析为 Vec ToolCall| RESP[ChatResponse]

    style CS fill:#E3F2FD
    style AS fill:#FFF3E0
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

```mermaid
sequenceDiagram
    participant U as 用户消息
    participant VI as VaultInjector.inject
    participant SP as scan_placeholders
    participant VS as VaultStore.get
    participant RP as replace_placeholders
    participant AS as AgentSession

    U->>VI: "使用 {{vault:api_key}} 调用 API"

    VI->>SP: 提取 {{vault:*}}
    SP-->>VI: ["api_key"]

    VI->>VS: .get("api_key")
    Note over VS: 可能解密

    VS-->>VI: "sk-xxxx"

    VI->>RP: 替换占位符
    RP-->>VI: "使用 sk-xxxx 调用 API"

    VI->>AS: 处理后的消息
    AS-->>U: 返回结果
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

```mermaid
flowchart TB
    CALLER[调用者]
    TS[TaskSpec::new<br/>id + prompt]
    SP[spawn_subagent<br/>provider + tools + workspace<br/>task + event_tx + result_tx<br/>token_tracker]
    TK[tokio::spawn<br/>AgentSession<br/>process_direct_streaming]
    SE[StreamEvent]
    CH[mpsc channel]
    ST[SubagentTracker]

    CALLER --> TS
    TS --> SP
    SP --> TK
    TK --> SE
    SE --> CH --> ST

    style SP fill:#E3F2FD
    style TK fill:#FFF3E0
```

### 7.2 Fire-and-Forget 模式

```mermaid
flowchart TB
    C[调用者]
    SA[spawn_subagent<br/>task + result_tx]
    JH[返回 JoinHandle]
    TS[tokio::spawn<br/>AgentSession<br/>process_direct]
    OM[OutboundMessage]

    C --> SA
    SA --> JH
    SA --> TS
    TS --> OM

    Note over TS: 10分钟超时

    OM -->|通过 outbound_tx<br/>发送到渠道| OUT[结果路由到 chat]

    style SA fill:#FFE0B2
```

### 7.3 同步等待模式

```mermaid
sequenceDiagram
    participant C as 调用者
    participant SA as spawn_subagent
    participant SP as tokio::spawn
    participant RT as result_tx.send

    C->>SA: task
    SA->>SP: AgentSession

    SP-->>SA: result
    SA->>RT: result
    RT-->>C: SubagentResult<br/>或 channel 关闭
```

---

## 8. 上下文压缩数据流

```mermaid
flowchart TB
    FR[finalize_response]
    PH[process_history]
    CC[ContextCompactor<br/>try_compact]

    FR --> PH --> CC

    CC --> TB{token_budget<br/>未超限?}

    TB -->|是| END([返回])
    TB -->|否| AS[异步执行]

    subgraph 压缩执行
        AS --> LL[LLM 生成摘要]
        LL --> ES[EventStore<br/>save_summary]
        ES --> SQ[SQLite 存储<br/>Summary 事件]
    end

    SQ --> END

    style CC fill:#FFE0B2
    style AS fill:#E3F2FD
```

### 压缩执行策略

- 非阻塞压缩在 `finalize_response` 中触发
- 压缩在后台执行，不阻塞响应
- 每次响应都会检查并执行压缩（如需要）

---

## 9. Hook 系统数据流

```mermaid
flowchart TB
    AS[AgentSession<br/>process_direct]

    subgraph Hook执行点
        BR[BeforeRequest Hook<br/>Sequential<br/>可修改/中止]
        AH[AfterHistory Hook<br/>Sequential<br/>可添加上下文]
        BL[BeforeLLM Hook<br/>Sequential<br/>Vault 注入等]
        AT[AfterToolCall Hook<br/>Parallel<br/>只读审计]
        AR[AfterResponse Hook<br/>Parallel<br/>只读审计]
    end

    AS --> BR
    BR --> SH[Load Session<br/>Save User Message]
    SH --> AH
    AH --> PH[Process History]
    PH --> BL
    BL --> LLM[LLM Provider]
    LLM --> AT
    AT --> RT[Return Response]
    RT --> AR
    AR --> SA[Save Assistant<br/>Message]

    style BR fill:#FFE0B2
    style AH fill:#E3F2FD
    style BL fill:#FFF3E0
    style AT fill:#F3E5F5
    style AR fill:#C8E6C9
```

### Hook 执行策略

| Hook Point | 策略 | 可修改 | 可中止 |
|------------|------|--------|--------|
| BeforeRequest | Sequential | ✓ | ✓ |
| AfterHistory | Sequential | ✓ | ✗ |
| BeforeLLM | Sequential | ✓ | ✗ |
| AfterToolCall | Parallel | ✗ | ✗ |
| AfterResponse | Parallel | ✗ | ✗ |
