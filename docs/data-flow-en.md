# Data Flow Design

> Data flow paths in Gasket-RS under different modes

---

## 1. CLI Mode Data Flow

```
User Input
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  reedline    │───▶│ AgentSession │───▶│   Prompt     │
│  (REPL)      │    │  .process_   │    │   Loader     │
│              │    │   direct()   │    │              │
└──────────────┘    └──────┬───────┘    └──────┬───────┘
                           │                    │
                    ┌──────▼───────┐     ┌──────▼───────┐
                    │   Session    │     │ Build System │
                    │   Manager    │     │ Prompt:      │
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
                  ┌─────▼──────┐   │    │  Final     │        │
                  │  Tool      │   │    │  Response  │        │
                  │  Executor  │   │    │  to User   │        │
                  │            │   │    └────────────┘        │
                  │ execute_   │   │                           │
                  │  batch()   │   │                           │
                  │ (parallel) │   │                           │
                  └─────┬──────┘   │                           │
                        │          │                           │
                  ┌─────▼──────┐   │                           │
                  │ Tool Result│   │                           │
                  │ append to  │   │                           │
                  │ messages   │───┘ (loop back to LLM Provider)
                  └────────────┘
```

---

## 2. Gateway Mode Data Flow (Actor Model)

```
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ Telegram │  │ Discord  │  │  Slack   │  │ Feishu   │  │ WebSocket│
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
                    │  (Single task, owns     │
                    │   routing table)        │
                    │                         │
                    │  HashMap<SessionKey,    │
                    │    mpsc::Sender>        │
                    │  • Route by session_key │
                    │  • Lazy Session Actor   │
                    │    creation             │
                    │  • Cleanup closed       │
                    │    channels             │
                    └────┬──────┬──────┬──────┘
                         │      │      │
              ┌──────────▼┐ ┌──▼────┐ ┌▼──────────┐
              │ Session   │ │Session│ │ Session   │
              │ Actor #1  │ │Act #2 │ │ Actor #N  │
              │           │ │       │ │           │
              │ Sequential│ ...     │ ...         │
              │ processing│ │       │ │           │
              │AgentSession│ │       │ │           │
              │ .process_ │ │       │ │           │
              │  direct() │ │       │ │           │
              │           │ │       │ │           │
              │ Idle timeout│       │ │           │
              │ auto-cleanup│       │ │           │
              └──────┬────┘ └──┬────┘ └─────┬─────┘
                     │         │            │
                     └────┬────┘────────────┘
                          │
              ┌───────────▼───────────┐
              │    Outbound Actor     │
              │  (Single task,        │
              │   dedicated sender)   │
              │                       │
              │  send_outbound()      │
              │  Route by channel type│
              └───┬──────┬──────┬────┘
                  │      │      │
        ┌─────────▼┐ ┌──▼────┐ ┌▼────────┐
        │ Telegram  │ │Slack  │ │WebSocket│  ...
        │  .send()  │ │.send()│ │ .send() │
        └───────────┘ └───────┘ └─────────┘
```

### Actor Model Design Points

| Actor | Responsibility | Concurrency Model |
|-------|----------------|-------------------|
| **Router Actor** | Distributes messages to Session Actors by SessionKey, lazy creation/cleanup | Single task, owns routing table HashMap, zero locks |
| **Session Actor** | Processes all messages for a single session serially, calls AgentSession | Independent tokio::spawn per session, shares `Arc<AgentSession>` |
| **Outbound Actor** | Cross-network HTTP/WebSocket sending, doesn't block upstream | Single task, external API blocking doesn't affect Agent |

---

## 3. Heartbeat & Cron Data Flow

```
┌─────────────────────────┐    ┌──────────────────────────┐
│  HeartbeatService       │    │  CronService              │
│                         │    │                            │
│  Read HEARTBEAT.md      │    │  Check SQLite every 60s   │
│  Parse cron expression  │    │  in cron_jobs table        │
│  Trigger time reached → │    │  Due tasks →               │
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
              │  (Gateway mode)  │
              │  or AgentSession │
              │  .process_direct │
              │  (CLI mode)      │
              └────────┬─────────┘
                       │
              ┌────────▼─────────┐
              │  Agent processes │
              │  (same as normal │
              │   messages)      │
              └──────────────────┘
```

---

## 4. Agent Execution Flowchart

```
                              ┌──────────────┐
                              │   Start      │
                              │  process_    │
                              │  direct()    │
                              └──────┬───────┘
                                     │
                              ┌──────▼───────┐
                              │ pre_request  │
                              │ Hook (opt)   │
                              │ Can modify/  │
                              │  abort       │
                              └──────┬───────┘
                                     │
                              ┌──────▼───────┐
                              │ Process slash│
                              │ commands     │
                              │ /new → clear │
                              │ /help → help │
                              └──────┬───────┘
                                     │ (not slash cmd)
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  1. Save user message to Session       │
                 │  2. Get history snapshot (memory_window)│
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  History Processor (token-aware)       │
                 │                                        │
                 │  Algorithm:                            │
                 │  1. Take last max_messages             │
                 │  2. Always keep last recent_keep       │
                 │  3. Earlier messages by token budget   │
                 │     include/evict                      │
                 │  → ProcessedHistory {                  │
                 │      messages: kept messages,          │
                 │      evicted: evicted messages         │
                 │    }                                   │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  ContextCompactor::compact()           │
                 │                                        │
                 │  evicted not empty → LLM summary       │
                 │  evicted empty → load existing summary │
                 │  → summary: Option<String>             │
                 └───────────────────┬───────────────────┘
                                     │
                 ┌───────────────────▼───────────────────┐
                 │  Prompt Assembly                       │
                 │                                        │
                 │  ┌──────────────────────────────────┐  │
                 │  │ [system] PROFILE.md + SOUL.md +  │  │
                 │  │          AGENTS.md + MEMORY.md + │  │
                 │  │          BOOTSTRAP.md +          │  │
                 │  │          skills_context          │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [assistant] Summary (if any)     │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [History messages × N] (processed)│  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] Current input             │  │
                 │  └──────────────────────────────────┘  │
                 └───────────────────┬───────────────────┘
                                     │
                              ┌──────▼───────┐
                              │ iteration = 0│
                              └──────┬───────┘
                                     │
                  ┌──────────────────▼──────────────────┐
            ┌─────│ iteration < max_iterations (def 20)?│
            │     └──────────────────┬──────────────────┘
            │ NO                     │ YES
            │                 ┌──────▼───────┐
            │                 │ iteration++  │
            │                 └──────┬───────┘
            │                        │
            │                 ┌──────▼───────────────────┐
            │                 │ Build ChatRequest:       │
            │                 │  model, messages, tools, │
            │                 │  temperature, max_tokens,│
            │                 │  thinking                │
            │                 └──────┬───────────────────┘
            │                        │
            │                 ┌──────▼───────────────────┐
            │                 │ LLM Provider.chat() /    │
            │                 │         .chat_stream()   │
            │                 │                          │
            │                 │ Fail → Exponential       │
            │                 │   backoff retry ×3       │
            │                 └──────┬───────────────────┘
            │                        │
            │                 ┌──────▼───────┐
            │                 │ ChatResponse │
            │                 └──────┬───────┘
            │                        │
            │              ┌─────────▼─────────┐
            │              │ has_tool_calls()? │
            │              └────┬──────────┬───┘
            │                   │ YES      │ NO
            │            ┌──────▼──────┐   │
            │            │ ToolExecutor│   │
            │            │.execute_    │   │
            │            │ batch()     │   │
            │            │             │   │
            │            │ spawn_parallel│   │
            │            │ + Execute all│   │
            │            │ tool_calls  │   │
            │            └──────┬──────┘   │
            │                   │          │
            │            ┌──────▼──────┐   │
            │            │ Append tool │   │
            │            │ results to  │   │
            │            │ messages    │   │
            │            └──────┬──────┘   │
            │                   │          │
            │                   ▼          │
            │           (back to loop top) │
            │                              │
            │                       ┌──────▼──────┐
            └──────────────────────▶│ Return final│
                                    │ response    │
                                    │ AgentResponse│
                                    │ {content,   │
                                    │  reasoning, │
                                    │  tools_used}│
                                    └──────┬──────┘
                                           │
                                    ┌──────▼───────┐
                                    │ post_response│
                                    │ Hook (opt)   │
                                    │ Audit/Alert  │
                                    └──────┬───────┘
                                           │
                                    ┌──────▼──────┐
                                    │ Save assistant│
                                    │ message to   │
                                    │ Session      │
                                    └─────────────┘
```

---

## 5. Streaming Output Flow

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
    Content(text)   Reasoning(text) (accumulate until stream ends)
           │            │            │
           ▼            ▼            ▼
    callback()      callback()    Parse to Vec<ToolCall>
    (real-time)     (real-time)   → ChatResponse
```

---

## 6. Vault Injection Flow

```
User message: "Connect with {{vault:api_key}}"
                    │
                    ▼
          ┌─────────────────┐
          │  VaultInjector  │
          │  .inject()      │
          └────────┬────────┘
                   │
         ┌─────────▼─────────┐
         │  scan_placeholders│
         │  extract {{vault:*}}│
         └─────────┬─────────┘
                   │
         ┌─────────▼─────────┐
         │   VaultStore      │
         │   .get(key)       │
         │   (may decrypt)   │
         └─────────┬─────────┘
                   │
         ┌─────────▼─────────┐
         │ replace_placeholders│
         │  with actual values │
         └─────────┬─────────┘
                   │
                   ▼
Processed message: "Connect with sk-xxxx"
                   │
                   ▼
            AgentSession processes
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

## 7. Event Sourcing Flow

### Event Persistence Flow

```
AgentSession::process_direct()
  │
  ├── User message ──▶ SessionEvent {
  │       event_type: UserMessage,
  │       parent_id: current_head,
  │       content: "user input",
  │       metadata: { branch: current_branch },
  │       ...
  │   } ──▶ SQLite storage
  │
  ├── Assistant response ──▶ SessionEvent {
  │       event_type: AssistantMessage,
  │       parent_id: previous_event_id,
  │       content: "assistant reply",
  │       metadata: { tools_used, token_usage },
  │       ...
  │   } ──▶ SQLite storage
  │
  ├── Tool calls ──▶ SessionEvent {
  │       event_type: ToolCall { tool_name, arguments },
  │       parent_id: assistant_event_id,
  │       content: JSON(args),
  │       ...
  │   } ──▶ SQLite storage
  │
  └── Tool results ──▶ SessionEvent {
          event_type: ToolResult { tool_call_id, tool_name, is_error },
          parent_id: tool_call_event_id,
          content: result_or_error,
          ...
      } ──▶ SQLite storage
```

### Branching and Version Control

```
Session Structure
┌─────────────────────────────────────────────────────────────┐
│  Session {                                                  │
│      key: "telegram:123:456",                               │
│      current_branch: "main",                                │
│      branches: HashMap {                                    │
│          "main"    ──▶ event_id_003,                        │
│          "feature" ──▶ event_id_007,                        │
│      },                                                     │
│      metadata: { ... }                                      │
│  }                                                          │
└─────────────────────────────────────────────────────────────┘

Event Chain (parent_id links)
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ event_001│────▶│ event_002│────▶│ event_003│     │ event_007│
│ UserMsg  │     │ ToolCall │     │ ToolRslt │     │ Summary  │
│ "hello"  │     │ exec()   │     │ "ok"     │     │ {...}    │
└──────────┘     └──────────┘     └──────────┘     └──────────┘
      │                                                   ^
      │                                                   │
      └───────────────────(main branch head)──────────────┘

Branch Creation
┌──────────┐     ┌──────────┐     ┌──────────┐
│ event_003│────▶│ event_004│────▶│ event_005│
│ ToolRslt │     │ UserMsg  │     │ AsstMsg  │
│          │     │ "feature"│     │ "done"   │
└──────────┘     └──────────┘     └──────────┘
      ^                                 │
      │                                 │
      └────(fork point)                 └────(feature branch head)

Merge Event
┌──────────┐
│ event_008│
│ Merge {  │
│   source_branch: "feature",
│   source_head: event_005,
│ } ──▶ combines feature into main
└──────────┘
```

### Summary Events

```
ContextCompactor::compact()
  │
  ├── Detect token budget exceeded
  │
  ├── Select evicted messages (oldest non-recent)
  │
  ├── Generate summary via LLM
  │   "3 messages about API design..."
  │
  └── Create SessionEvent {
          event_type: Summary {
              summary_type: Compression { token_budget: 4000 },
              covered_event_ids: [event_001, event_002, event_003],
          },
          content: "summary text",
          parent_id: last_evicted_event_id,
          ...
      } ──▶ SQLite storage

Query with Summary
┌─────────────────────────────────────────────────────────────┐
│  Build Prompt:                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ [Summary event] "3 messages about API design..."    │    │
│  │ (replaces evicted messages in context window)       │    │
│  ├─────────────────────────────────────────────────────┤    │
│  │ [Recent messages] (kept within token budget)        │    │
│  ├─────────────────────────────────────────────────────┤    │
│  │ [Current user message]                              │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

---

## 8. Subagent Spawning Patterns

```
─── spawn_subagent() (async fire-and-forget) ───

Caller ──▶ spawn_subagent(task, result_tx, ...)
  │              │
  │  Returns     │  tokio::spawn
  │  JoinHandle  │  │
  │              ▼  │
  │        AgentSession::process_direct()
  │              │
  ▼              ▼
(don't wait)  OutboundMessage ──▶ channel


─── Sync wait via channel ───

Caller ──▶ spawn_subagent(task, result_tx, ...)
  │              │
  │  await rx    │  tokio::spawn
  │  (blocking)  │  │
  ▼              ▼  │
(receives     AgentSession::process_direct()
 SubagentResult)   │
                    ▼
              result_tx.send(result)
```
