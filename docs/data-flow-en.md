# Data Flow Design

> Data flow paths in Gasket-RS under different modes

---

## 1. CLI Mode Data Flow

```mermaid
flowchart TB
    subgraph User_Input
        U[User Input]
    end

    subgraph REPL_Layer
        R[reedline REPL]
    end

    subgraph AgentSession
        AS[AgentSession.process_direct]
    end

    subgraph Context_Assembly
        PL[Prompt Loader]
        AC[Session Manager<br/>SQLite]
        SYS[Build System Prompt<br/>PROFILE.md +<br/>SOUL.md +<br/>AGENTS.md +<br/>MEMORY.md +<br/>BOOTSTRAP.md + skills]
    end

    subgraph LLM_Call
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

    TC -->|YES| TE[Tool Executor<br/>execute_batch<br/>parallel]
    TE --> TR[Tool Result<br/>append to messages]
    TR --> LLM

    TC -->|NO| OUT[Final Response<br/>to User]

    style AS fill:#E3F2FD
    style LLM fill:#FFF3E0
    style TE fill:#F3E5F5
```

---

## 2. Gateway Mode Data Flow (Actor Model)

```mermaid
flowchart TB
    subgraph User_Entry
        TG[Telegram Bot]
        DC[Discord Bot]
        SL[Slack WSS]
        FS[Feishu Webhook]
        WS[WebSocket Server]
    end

    subgraph Message_Processing
        IM[InboundMessage<br/>channel + sender_id<br/>chat_id + content<br/>media + metadata]
        MW[Middleware Layer<br/>Auth Check + Rate Limiter]
        RT[Router Actor<br/>HashMap SessionKey<br/>mpsc Sender<br/>Lazy Session creation/cleanup]
    end

    subgraph Session_Layer
        S1[Session Actor #1]
        S2[Session Actor #2]
        SN[Session Actor #N]
    end

    subgraph Outbound_Layer
        OB[Outbound Actor<br/>Single task<br/>Dedicated sender<br/>send_outbound]
        OT[Reply Telegram]
        OS[Reply Slack]
        OW[Reply WebSocket]
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

### Actor Model Design Points

| Actor | Responsibility | Concurrency Model |
|-------|----------------|-------------------|
| **Router Actor** | Distributes messages to Session Actors by SessionKey, lazy creation/cleanup | Single task, owns routing table HashMap, zero locks |
| **Session Actor** | Processes all messages for a single session serially, calls AgentSession | Independent tokio::spawn per session, shares `Arc<AgentSession>` |
| **Outbound Actor** | Cross-network HTTP/WebSocket sending, doesn't block upstream | Single task, external API blocking doesn't affect Agent |

---

## 3. Heartbeat & Cron Data Flow

```mermaid
flowchart TB
    subgraph Scheduled_Tasks
        HB[HeartbeatService<br/>Read HEARTBEAT.md<br/>Parse cron expression<br/>Trigger time reached]
        CS[CronService<br/>Check SQLite every 60s<br/>in cron_jobs table<br/>Due tasks trigger]
    end

    subgraph Message_Generation
        IM1[InboundMessage<br/>sender_id: heartbeat<br/>content: task_text]
        IM2[InboundMessage<br/>sender_id: cron<br/>content: job.message]
    end

    IM1 --> HB
    IM2 --> CS

    HB --> RT[Router Actor<br/>Gateway mode]
    CS --> RT

    RT --> AG[Agent processes<br/>same as normal<br/>messages]

    style HB fill:#FFE0B2
    style CS fill:#E3F2FD
```

---

## 4. Agent Execution Flowchart

```mermaid
flowchart TB
    START([Start]) --> PR[process_direct]

    PR --> BR[pre_request Hook<br/>Optional<br/>Can modify/abort]

    BR --> SL[Process slash commands<br/>/new → clear<br/>/help → help]

    SL --> SM{slash cmd?}

    SM -->|YES| EX[Execute command]
    SM -->|NO| SH

    subgraph Save_Message
        SH[1. Save user message<br/>to SessionEvent]
    end

    SH --> HH[History Processor<br/>token-aware]

    subgraph History_Processing
        HH --> HP[Algorithm:<br/>1. Take last max_messages<br/>2. Always keep last recent_keep<br/>3. Earlier messages by token budget<br/>→ ProcessedHistory<br/>messages + evicted]
    end

    HP --> CC[ContextCompactor<br/>compact]

    CC --> EV{evicted<br/>not empty?}

    EV -->|YES| SUM[LLM summary]
    EV -->|NO| LS[Load existing summary]
    SUM --> SS[summary: Option String]
    LS --> SS

    SS --> PA[Prompt Assembly]

    subgraph Prompt_Assembly
        PA --> SYS1["[system] PROFILE.md + SOUL.md +<br/>AGENTS.md + BOOTSTRAP.md +<br/>skills_context"]
        PA --> USR1["[user] [SYSTEM: dynamic memory]<br/>Relevant memories +<br/>summary (if any)"]
        PA --> USR2["[assistant/user] History<br/>messages (processed)"]
        PA --> USR3["[user] Current input"]
    end

    PA --> I[iteration = 0]

    I --> LP{iteration &lt;<br/>max_iterations<br/>(default 20)?}

    LP -->|YES| INC[iteration++]
    INC --> CR[Build ChatRequest<br/>model + messages + tools +<br/>temperature + max_tokens +<br/>thinking]

    CR --> LLM[LLM Provider<br/>chat / chat_stream]

    LLM --> LR{Fail?}

    LR -->|YES| RET[Exponential backoff<br/>retry ×3]
    RET --> LLM

    LR -->|NO| CR2[ChatResponse]

    CR2 --> TC{has_tool<br/>_calls?}

    TC -->|YES| TE[ToolExecutor<br/>execute_batch<br/>parallel]

    TE --> TR[Append tool<br/>results to messages]

    TR --> I

    TC -->|NO| OUT[Return final<br/>response<br/>AgentResponse<br/>content + reasoning<br/>+ tools_used]

    LP -->|NO| OUT

    OUT --> AR[post_response Hook<br/>Optional<br/>Audit/Alert]

    AR --> SA[Save assistant<br/>message to<br/>Session]

    SA --> END([Done])

    style PR fill:#E3F2FD
    style LLM fill:#FFF3E0
    style TE fill:#F3E5F5
```

---

## 5. Streaming Output Flow

```mermaid
flowchart TB
    CS[chat_stream]
    AS[accumulate_stream]
    CH[StreamEvent]

    subgraph delta_processing
        DC[delta.content]
        DR[delta.reasoning]
        DT[delta.tool_calls]
    end

    subgraph Event_Conversion
        EC[StreamEvent<br/>Content text]
        ER[StreamEvent<br/>Reasoning text]
        ET[tool_calls_map<br/>accumulate until stream ends]
    end

    CS --> AS --> DC
    AS --> DR
    AS --> DT

    DC --> EC
    DR --> ER
    DT --> ET

    EC --> CB[callback<br/>real-time]
    ER --> CB
    ET -->|Parse to Vec ToolCall| RESP[ChatResponse]

    style CS fill:#E3F2FD
    style AS fill:#FFF3E0
```

---

## 6. Vault Injection Flow

```mermaid
sequenceDiagram
    participant U as User message
    participant VI as VaultInjector.inject
    participant SP as scan_placeholders
    participant VS as VaultStore.get
    participant RP as replace_placeholders
    participant AS as AgentSession

    U->>VI: "Connect with {{vault:api_key}}"

    VI->>SP: Extract {{vault:*}}
    SP-->>VI: ["api_key"]

    VI->>VS: .get("api_key")
    Note over VS: May decrypt

    VS-->>VI: "sk-xxxx"

    VI->>RP: Replace placeholders
    RP-->>VI: "Connect with sk-xxxx"

    VI->>AS: Processed message
    AS-->>U: Return result
```

### InjectionReport

```rust
InjectionReport {
    total_placeholders: 1,
    replaced: 1,
    missing_keys: [],      // Keys not found are recorded here
}
```

---

## 7. Subagent Spawning Patterns

### Pure Function Creation (Recommended)

```mermaid
flowchart TB
    CALLER[Caller]
    TS[TaskSpec::new<br/>id + prompt]
    SP[spawn_subagent<br/>provider + tools + workspace<br/>task + event_tx + result_tx<br/>token_tracker + cancellation_token]
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

### Fire-and-Forget Mode

```mermaid
flowchart TB
    C[Caller]
    SA[spawn_subagent<br/>task + result_tx<br/>+ cancellation_token]
    JH[Returns JoinHandle]
    TS[tokio::spawn<br/>AgentSession<br/>process_direct]
    OM[OutboundMessage]

    C --> SA
    SA --> JH
    SA --> TS
    TS --> OM

    Note over TS: 10-minute timeout

    OM -->|via outbound_tx<br/>sent to channel| OUT[Result routed to chat]

    style SA fill:#FFE0B2
```

### Sync Wait Mode

```mermaid
sequenceDiagram
    participant C as Caller
    participant SA as spawn_subagent
    participant SP as tokio::spawn
    participant RT as result_tx.send

    C->>SA: task
    SA->>SP: AgentSession

    SP-->>SA: result
    SA->>RT: result
    RT-->>C: SubagentResult<br/>or channel closed
```

---

## 8. Context Compaction Flow

```mermaid
flowchart TB
    FR[finalize_response]
    PH[process_history]
    CC[ContextCompactor<br/>try_compact]

    FR --> PH --> CC

    CC --> TB{token_budget<br/>not exceeded?}

    TB -->|YES| END([Return])
    TB -->|NO| AS[Async execution]

    subgraph Compaction_Execution
        AS --> LL[LLM generates summary]
        LL --> ES[EventStore<br/>save_summary]
        ES --> SQ[SQLite stores<br/>Summary event]
    end

    SQ --> END

    style CC fill:#FFE0B2
    style AS fill:#E3F2FD
```

### Compaction Execution Strategy

- Non-blocking compaction triggered in `finalize_response`
- Compaction runs in background, does not block response
- Checked after every response (if needed)

---

## 9. Hook System Data Flow

```mermaid
flowchart TB
    AS[AgentSession<br/>process_direct]

    subgraph Hook_Execution_Points
        BR[BeforeRequest Hook<br/>Sequential<br/>Can modify/abort]
        AH[AfterHistory Hook<br/>Sequential<br/>Can add context]
        BL[BeforeLLM Hook<br/>Sequential<br/>Vault injection etc]
        AT[AfterToolCall Hook<br/>Parallel<br/>Read-only audit]
        AR[AfterResponse Hook<br/>Parallel<br/>Read-only audit]
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

### Hook Execution Strategy

| Hook Point | Strategy | Can Modify? | Can Abort? |
|------------|----------|-------------|------------|
| BeforeRequest | Sequential | ✓ | ✓ |
| AfterHistory | Sequential | ✓ | ✗ |
| BeforeLLM | Sequential | ✓ | ✗ |
| AfterToolCall | Parallel | ✗ | ✗ |
| AfterResponse | Parallel | ✗ | ✗ |
