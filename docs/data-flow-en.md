# Data Flow Design

> Data flow paths in Gasket-RS under different modes

---

## 1. CLI Mode Data Flow

```
User Input
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  reedline    │───▶│  AgentLoop   │───▶│   Prompt     │
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
              │ AgentLoop │ │       │ │           │
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
| **Session Actor** | Processes all messages for a single session serially, calls AgentLoop | Independent tokio::spawn per session, shares `Arc<AgentLoop>` |
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
              │  or AgentLoop    │
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
                 │  ContextCompressionHook.compress()     │
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
            │            │ Execute all │   │
            │            │ tool_calls  │   │
            │            │ in parallel │   │
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
            AgentLoop processes
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

## 7. SubagentManager Scheduling Patterns

```
─── submit() (async fire-and-forget) ───

Caller ──▶ tokio::spawn ──▶ AgentLoop.process_direct() ──▶ OutboundMessage
  │                              │                              │
  │  Returns Ok(()) immediately  │  10 min timeout             │  via outbound_tx
  │                              │                              │  to channel
  ▼                              ▼                              ▼
(don't wait)               (background run)              (result routed to chat)


─── submit_and_wait() (sync wait) ───

Caller ──▶ tokio::spawn ──▶ AgentLoop.process_direct() ──▶ oneshot::Sender
  │              │                                              │
  │  await rx    │  10 min timeout                               │ tx.send(result)
  │  (blocking)  │                                              │
  ▼              ▼                                              ▼
(receives AgentResponse                                 (oneshot channel)
 or Error)
```
