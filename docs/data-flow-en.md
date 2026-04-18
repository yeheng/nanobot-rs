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
                 │  │          AGENTS.md + BOOTSTRAP.  │  │
                 │  │          md + skills_context     │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] [SYSTEM: dynamic memory]  │  │
                 │  │        Relevant memories +       │  │
                 │  │        summary (if any)          │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [assistant/user] History         │  │
                 │  │        messages (processed)      │  │
                 │  ├──────────────────────────────────┤  │
                 │  │ [user] Current input             │  │
                 │  └──────────────────────────────────┘  │
                 │                                        │
                 │  Note: Dynamic memory is injected as   │
                 │  a User Message to preserve Prompt     │
                 │  Cache for static system content.      │
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
    Content(text)   Thinking(text)  (accumulate until stream ends)
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

## 7. Subagent Spawning Patterns

### Pure Function Creation (Recommended)

```
Caller ──▶ TaskSpec::new(id, prompt)
              │
              ├──▶ .with_model()
              ├──▶ .with_system_prompt()
              │
              ▼
    spawn_subagent(provider, tools, workspace, task, event_tx, result_tx, token_tracker, cancellation_token)
              │
              ▼
    tokio::spawn(async {
        AgentSession::process_direct_streaming()
    })
              │
              ▼
    StreamEvent ──▶ mpsc::channel ──▶ SubagentTracker
```

### Fire-and-Forget Mode

```
Caller ──▶ spawn_subagent(task, result_tx, ..., cancellation_token)
  │
  │  Returns JoinHandle
  │
  ▼
tokio::spawn ──▶ AgentSession::process_direct() ──▶ OutboundMessage
                     │                              │
                     │  10-minute timeout           │  via outbound_tx
                     │                              │  sent to channel
                     ▼                              ▼
               (runs in background)          (result routed to chat)
```

### Sync Wait Mode

```
Caller ──▶ spawn_subagent(task, result_tx, ..., cancellation_token)
  │              │
  │  await rx    │  tokio::spawn
  │  (blocking)  │  │
  ▼              ▼  │
(receives     AgentSession::process_direct()
 SubagentResult)   │
                    ▼
              result_tx.send(result)
                    │
                    ▼
              (returns result to caller)
```

---

## 8. Context Compaction Flow

```
finalize_response()
    │
    ▼
process_history() ──▶ identify evicted messages
    │
    ▼
ContextCompactor::try_compact(key, current_tokens)
    │
    ├──▶ token_budget not exceeded? ──▶ return
    │
    ▼
async execution {
    │
    ▼
    LLM generates summary
    │
    ▼
    EventStore::save_summary()
    │
    ▼
    SQLite stores Summary event
}
```

### Compaction Execution Strategy

- Non-blocking compaction triggered in `finalize_response`
- Compaction runs in background, does not block response
- Checked after every response (if needed)

---

## 9. Hook System Data Flow

```
AgentSession::process_direct()
    │
    ├──▶ BeforeRequest Hook ──▶ can modify/abort request
    │
    ▼
Load Session / Save User Message
    │
    ├──▶ AfterHistory Hook ──▶ can add context
    │
    ▼
Process History
    │
    ├──▶ BeforeLLM Hook ──▶ last-chance modifications (e.g., Vault injection)
    │
    ▼
LLM Provider
    │
    ├──▶ AfterToolCall Hook ──▶ parallel execution, read-only audit
    │
    ▼
Return Response
    │
    ├──▶ AfterResponse Hook ──▶ parallel execution, read-only audit
    │
    ▼
Save Assistant Message
```

### Hook Execution Strategy

| Hook Point | Strategy | Can Modify? | Can Abort? |
|------------|----------|-------------|------------|
| BeforeRequest | Sequential | ✓ | ✓ |
| AfterHistory | Sequential | ✓ | ✗ |
| BeforeLLM | Sequential | ✓ | ✗ |
| AfterToolCall | Parallel | ✗ | ✗ |
| AfterResponse | Parallel | ✗ | ✗ |
