# Agent Module Architecture

> **Linus-style Architecture Review**: Good code should be self-explanatory, but complex systems need blueprints. This document is the "source code map" for the agent module.

---

## 1. High-Level Data Flow Overview

```mermaid
flowchart TB
    subgraph External["External Input"]
        User[User Message]
        Hook[Shell Hooks<br/>~/.gasket/hooks/]
        Vault[Vault Secrets]
    end

    subgraph AgentCore["Agent Core - loop_.rs"]
        AL[AgentLoop]
        AC[AgentContext Enum]
        PC[PersistentContext]
        SC[Stateless]
    end

    subgraph Execution["Execution Layer - executor_core.rs"]
        AE[AgentExecutor]
        TE[ToolExecutor]
        RH[RequestHandler]
    end

    subgraph Subagent["Subagent System"]
        SM[SubagentManager]
        ST[SubagentTracker]
        SPT[SpawnParallelTool]
    end

    subgraph Output["Output Layer"]
        OB[Outbound Actor]
        WS[WebSocket Stream]
    end

    User --> AL
    Hook -.->|pre_request| AL
    Hook -.->|post_response| AL
    Vault -.->|inject| AL

    AL -->|delegate| AE
    AE --> TE
    AE --> RH

    AL -.->|uses| AC
    AC -.->|impl| PC
    AC -.->|impl| SC

    AE -.->|can spawn| SM
    SM -.->|tracked by| ST
    SPT -.->|orchestrates| SM

    AL -->|send| OB
    SM -->|send| OB
    OB -->|stream| WS
```

---

## 2. AgentLoop Execution Flow Details

```mermaid
flowchart LR
    subgraph Phase1["Phase 1: Preprocessing"]
        A[pre_request Hook] --> B[Load Session]
        B --> C[Save User Message]
    end

    subgraph Phase2["Phase 2: History Processing"]
        D[Process History] --> E{Has evicted?}
        E -->|Yes| F[Background Compression]
        E -->|No| G[Load Summary]
        F --> G
    end

    subgraph Phase3["Phase 3: Prompt Assembly"]
        H[System Prompt] --> I[Skills Context]
        I --> J[Assemble Messages]
        J --> K[Vault Injection]
    end

    subgraph Phase4["Phase 4: LLM Execution"]
        L[AgentExecutor] --> M{Tool Calls?}
        M -->|Yes| N[Execute Tools]
        N --> L
        M -->|No| O[Return Response]
    end

    subgraph Phase5["Phase 5: Post-processing"]
        P[post_response Hook] --> Q[Save Assistant Message]
    end

    Phase1 --> Phase2 --> Phase3 --> Phase4 --> Phase5
```

---

## 3. Subagent Concurrency Model

```mermaid
sequenceDiagram
    participant Main as Main Agent
    participant SPT as SpawnParallelTool
    participant ST as SubagentTracker
    participant SM as SubagentManager
    participant Sub as Subagent Task
    participant OB as Outbound Channel

    Main->>SPT: execute(tasks)
    SPT->>ST: new()
    SPT->>ST: result_sender()
    SPT->>ST: event_sender()

    loop For each task
        SPT->>SM: submit_tracked_streaming()
        SM->>Sub: tokio::spawn(async)
        Sub->>Sub: AgentLoop::process_direct()
    end

    par Result Collection
        ST->>ST: wait_for_all()
    and Event Streaming
        Sub-->>ST: SubagentEvent::Thinking
        Sub-->>ST: SubagentEvent::ToolStart
        Sub-->>ST: SubagentEvent::ToolEnd
        Sub-->>ST: SubagentEvent::Completed
    and WebSocket Forward
        ST-->>OB: Forward events
    end

    ST->>SPT: Vec<SubagentResult>
    SPT->>Main: Aggregated output
```

---

## 4. Key Data Structure Relationships

```mermaid
classDiagram
    class AgentLoop {
        +provider: Arc~LlmProvider~
        +tools: Arc~ToolRegistry~
        +config: AgentConfig
        +context: Arc~AgentContext~
        +system_prompt: String
        +vault_injector: Option~VaultInjector~
        +pricing: Option~ModelPricing~
        +hook_registry: Arc~HookRegistry~
        +process_direct()
        +process_direct_streaming()
        +process_direct_streaming_with_channel()
        -run_agent_loop()
    }

    class ContextCompactor {
        +provider: Arc~LlmProvider~
        +event_store: Arc~EventStore~
        +model: String
        +token_budget: usize
        +compaction_threshold: f32
        +summarization_prompt: String
        +compress()
        +load_summary()
        +recall_history()
    }

    class AgentContext {
        <<enumeration>>
        +Persistent(PersistentContext)
        +Stateless
    }

    class PersistentContext {
        +event_store: Arc~EventStore~
        +sqlite_store: Arc~SqliteStore~
        +embedder: Option~Arc~TextEmbedder~~
    }

    class Stateless {
        +no-op implementations
    }

    class AgentExecutor {
        +provider: Arc~LlmProvider~
        +tools: Arc~ToolRegistry~
        +config: ~AgentConfig~
        +execute()
        +execute_stream()
        -handle_tool_calls()
    }

    class SubagentManager {
        +provider: Arc~LlmProvider~
        +tools: Arc~ToolRegistry~
        +outbound_tx: mpsc::Sender
        +session_key: RwLock~Option~SessionKey~~
        +timeout_secs: u64
        +task() SubagentTaskBuilder
        +submit()
        +submit_and_wait()
        +submit_and_wait_with_model()
        +submit_and_wait_with_model_streaming()
    }

    class SubagentTaskBuilder {
        +subagent_id: String
        +task: String
        +provider: Option~Arc~LlmProvider~~
        +config: Option~AgentConfig~
        +event_tx: Option~mpsc::Sender~
        +system_prompt: Option~String~
        +session_key: Option~SessionKey~
        +cancellation_token: Option~CancellationToken~
        +hooks: Option~Arc~HookRegistry~~
        +with_provider()
        +with_config()
        +with_streaming()
        +with_system_prompt()
        +with_cancellation_token()
        +spawn()
    }

    class SubagentTracker {
        +results: Arc~RwLock~HashMap~~
        +result_tx: mpsc::Sender
        +event_tx: mpsc::Sender
        +wait_for_all()
        +get_result()
        +drain_events()
    }

    AgentLoop --> AgentContext : uses
    AgentContext ..> PersistentContext : variant
    AgentContext ..> Stateless : variant
    PersistentContext --> ContextCompactor : uses
    AgentLoop --> AgentExecutor : delegates
    SubagentManager ..> AgentLoop : creates
    SubagentManager --> SubagentTracker : uses
    SubagentManager ..> SubagentTaskBuilder : creates
```

---

## 5. Execution Mode Comparison

| Mode | Context Type | Persistence | Typical Use | Entry Point |
|------|-----------|--------|---------|--------|
| **Main Agent** | AgentContext::Persistent | Yes | User conversation | `AgentLoop::new()` |
| **Background Subagent** | AgentContext::Stateless | No | Background tasks | `SubagentManager::submit()` |
| **Sync Subagent** | AgentContext::Stateless | No | Governance agent | `SubagentManager::submit_and_wait()` |
| **Parallel Subagent** | AgentContext::Stateless | No | Parallel computation | `SpawnParallelTool::execute()` |
| **Model Switch** | AgentContext::Stateless | No | Switch model | `SubagentManager::submit_and_wait_with_model()` |

---

## 6. Key Execution Path Code Mapping

### 6.1 Main Agent Execution Path

```
User Input
    ↓
AgentLoop::process_direct() [loop_.rs:440]
    ↓
AgentLoop::run_agent_loop() [loop_.rs:735]
    ↓
AgentExecutor::execute_with_options() [executor_core.rs:152]
    ↓
RequestHandler::send_with_retry() [request.rs]
    ↓
LlmProvider::chat_stream()
```

### 6.2 Subagent Execution Path

```
Tool Call (spawn_parallel)
    ↓
SpawnParallelTool::execute() [spawn_parallel.rs]
    ↓
SubagentTracker::new() + event_sender()
    ↓
SubagentManager::task(id, prompt) → SubagentTaskBuilder [subagent.rs]
    ↓
SubagentTaskBuilder::with_streaming().spawn()
    ↓
tokio::spawn(async { ... })
    ↓
AgentLoop::builder() → AgentContext::Stateless [loop_.rs]
    ↓
AgentLoop::process_direct_streaming()
    ↓
Result → mpsc::channel → SubagentTracker
```

---

## 7. Design Review: Potential Issues and Risks

### 7.1 🟢 Low Risk: Subagent Result Loss (Mitigated)

**Previous Issue**: `SubagentTracker::wait_for_all()` uses `tokio::time::timeout`, but results from partially completed tasks may be lost after timeout.

**Mitigation**: `CancellationToken` is now used for graceful cancellation of timed-out tasks.

**Code Location**: `subagent_tracker.rs`

```rust
// Now uses CancellationToken for graceful cancellation
pub async fn wait_for_all_timeout(&self, count: usize, timeout: Duration) -> Vec<SubagentResult> {
    // SubagentTaskBuilder::with_cancellation_token() ensures clean shutdown
}
```

**Status**: ✅ Mitigated with CancellationToken pattern

### 7.2 🟡 Medium Risk: Channel Backpressure

**Issue**: Event forwarding task in `spawn_parallel.rs:292-360` uses infinite loop; if WebSocket consumer is slower than producer, may cause memory growth.

**Code Location**: `spawn_parallel.rs:354`

```rust
// Uses try_send to avoid blocking, but only warns on failure
if let Err(e) = outbound_tx.try_send(outbound) {
    warn!("Failed to send subagent event to outbound channel: {}", e);
}
```

**Recommendation**: Consider using bounded channel + backpressure strategy, or rate-limited sending.

### 7.3 🟡 Medium Risk: Task-Local Storage Abuse Risk

**Issue**: `CURRENT_SESSION_KEY` is global task-local variable; while currently controlled, adds implicit dependencies.

**Code Location**: `loop_.rs:81-83`

```rust
task_local! {
    pub static CURRENT_SESSION_KEY: Option<SessionKey>;
}
```

**Current Mitigation**:
- Detailed comments explaining usage restrictions
- Only used in Tool::execute() to get session context
- Prohibited for storing mutable state

### 7.4 🟢 Low Risk: Code Duplication

**Issue**: `SubagentManager` has multiple similar submit methods with code duplication.

**Code Location**: `subagent.rs:90-331`

- `submit()` - fire-and-forget
- `submit_and_wait()` - sync wait
- `submit_and_wait_with_model()` - with model switch
- `submit_and_wait_with_model_streaming()` - with streaming

**Recommendation**: Consider using builder pattern or unified parameter struct to reduce duplication.

---

## 8. Taste Score

```
┌─────────────────────────────────────────────────────────┐
│  【Taste Score】 Good Taste ✓                            │
├─────────────────────────────────────────────────────────┤
│  【Highlights】                                          │
│  • AgentContext enum eliminates trait object overhead   │
│  • Clear separation between execution and state layers   │
│  • Vault values at request-level scope, prevents leaks   │
│  • ContextCompactor for synchronous context compression  │
│  • Builder pattern for subagent task configuration       │
├─────────────────────────────────────────────────────────┤
│  【Improvements】                                        │
│  • Subagent timeout handling now uses CancellationToken  │
│  • Builder pattern unifies subagent API                  │
│  • ContextCompactor for sync compression (was async bg task) │
└─────────────────────────────────────────────────────────┘
```

---

## 9. File Index

| File | Responsibility | Key Structures |
|------|------|---------|
| `loop_.rs` | Main Agent loop | `AgentLoop`, `AgentConfig`, `AgentResponse` |
| `executor_core.rs` | Core execution engine | `AgentExecutor`, `ExecutionResult` |
| `executor.rs` | Execution facade | `AgentExecutor`, `ToolExecutor` |
| `context.rs` | Context management | `AgentContext` enum, `PersistentContext`, `Stateless` |
| `subagent.rs` | Subagent management | `SubagentManager`, `SubagentTaskBuilder`, `SessionKeyGuard` |
| `subagent_tracker.rs` | Parallel tracking | `SubagentTracker`, `SubagentEvent`, `SubagentResult` |
| `spawn_parallel.rs` | Parallel tool | `SpawnParallelTool` |
| `pipeline.rs` | Simplified pipeline | `process_message()` |
| `stream.rs` | Stream events | `StreamEvent` |
| `request.rs` | Request building | `RequestHandler` |
| `processor.rs` | History processing | `process_history()`, `HistoryConfig`, `ProcessedHistory` |
| `compactor.rs` | Context compression | `ContextCompactor`, `CompactionConfig` |
| `prompt.rs` | Prompt loading | `load_prompt()`, `load_skills_context()` |
