# Agent Module Architecture

> Agent 模块技术架构文档

---

## 1. 高层数据流概览

```mermaid
flowchart TB
    subgraph External["外部输入"]
        User[用户消息]
        Hook[Pipeline Hooks<br/>~/.gasket/hooks/]
        Vault[Vault Secrets]
    end

    subgraph AgentCore["Agent Core - session/mod.rs"]
        AL[AgentSession]
        AC[AgentContext 枚举]
        PC[PersistentContext]
        SC[Stateless]
    end

    subgraph Execution["执行层 - kernel/executor.rs"]
        AE[AgentExecutor]
        TE[ToolExecutor]
    end

    subgraph Subagent["子代理系统"]
        SM[SubagentManager]
        ST[SubagentTracker]
        SPT[SpawnParallelTool]
    end

    subgraph Output["输出层"]
        OB[Outbound Actor]
        WS[WebSocket Stream]
    end

    User --> AL
    Hook -.->|BeforeRequest| AL
    Hook -.->|AfterResponse| AL
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

## 2. AgentSession 执行流程详解

```mermaid
flowchart LR
    subgraph Phase1["阶段1: 预处理"]
        A[BeforeRequest Hook] --> B[Load Session]
        B --> C[Save User Message]
    end

    subgraph Phase2["阶段2: 历史处理"]
        D[Process History] --> E{有evicted?}
        E -->|Yes| F[Context Compression]
        E -->|No| G[Load Summary]
        F --> G
    end

    subgraph Phase3["阶段3: 提示组装"]
        H[System Prompt] --> I[Skills Context]
        I --> J[Assemble Messages]
        J --> K[Vault Injection]
    end

    subgraph Phase4["阶段4: Kernel执行"]
        L[kernel::execute] --> M{Tool Calls?}
        M -->|Yes| N[ToolExecutor]
        N --> L
        M -->|No| O[Return ExecutionResult]
    end

    subgraph Phase5["阶段5: 后处理"]
        P[AfterResponse Hook] --> Q[Save Assistant Message]
    end

    Phase1 --> Phase2 --> Phase3 --> Phase4 --> Phase5
```

---

## 3. Subagent 并发模型

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
        SPT->>SM: task(id, prompt).with_streaming().spawn()
        SM->>Sub: tokio::spawn(async)
        Sub->>Sub: AgentSession::process_direct_streaming()
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

## 4. 关键数据结构关系

```mermaid
classDiagram
    class AgentSession {
        +runtime_ctx: RuntimeContext
        +context: AgentContext
        +config: AgentConfig
        +workspace: PathBuf
        +system_prompt: String
        +skills_context: Option~String~
        +hooks: Arc~HookRegistry~
        +compactor: Option~Arc~ContextCompactor~~
        +memory_manager: Option~Arc~MemoryManager~~
        +process_direct()
        +process_direct_streaming_with_channel()
        -prepare_pipeline()
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

    class ContextCompactor {
        +provider: Arc~LlmProvider~
        +event_store: Arc~EventStore~
        +sqlite_store: Arc~SqliteStore~
        +model: String
        +token_budget: usize
        +try_compact()
    }

    class AgentExecutor {
        +provider: Arc~LlmProvider~
        +tools: Arc~ToolRegistry~
        +config: ~KernelConfig~
        +execute_with_options()
        +execute_stream_with_options()
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

    AgentSession --> AgentContext : uses
    AgentContext ..> PersistentContext : variant
    AgentContext ..> Stateless : variant
    PersistentContext --> ContextCompactor : uses
    AgentSession --> AgentExecutor : delegates via kernel
    SubagentManager --> SubagentTracker : uses
```

---

## 5. 执行模式对比

| 模式 | 上下文类型 | 持久化 | 典型用途 | 入口点 |
|------|-----------|--------|---------|--------|
| **Main Agent** | AgentContext::Persistent | 是 | 用户对话 | `AgentSession::new()` |
| **Background Subagent** | AgentContext::Stateless | 否 | 后台任务 | `SubagentManager::submit()` |
| **Sync Subagent** | AgentContext::Stateless | 否 | 治理代理 | `SubagentManager::submit_and_wait()` |
| **Parallel Subagent** | AgentContext::Stateless | 否 | 并行计算 | `SpawnParallelTool::execute()` |
| **Model Switch** | AgentContext::Stateless | 否 | 切换模型 | `SubagentManager::submit_and_wait_with_model()` |

---

## 6. 关键执行路径代码映射

### 6.1 主Agent执行路径
```
User Input
    ↓
AgentSession::process_direct() [session/mod.rs]
    ↓
prepare_pipeline() → BuildOutcome::Ready
    ↓
kernel::execute() [kernel/mod.rs]
    ↓
AgentExecutor::execute_with_options() [kernel/executor.rs]
    ↓
LlmProvider::chat_stream()
    ↓
finalize_response() → AgentResponse
```

### 6.2 流式执行路径
```
User Input
    ↓
AgentSession::process_direct_streaming_with_channel() [session/mod.rs]
    ↓
prepare_pipeline() → BuildOutcome::Ready
    ↓
kernel::execute_streaming() [kernel/mod.rs]
    ↓
AgentExecutor::execute_stream_with_options() [kernel/executor.rs]
    ↓
StreamEvent::Content/Reasoning/ToolStart/ToolEnd → mpsc::channel
    ↓
finalize_response() → AgentResponse
```

### 6.3 Subagent执行路径（Builder模式）
```
Tool Call (spawn_parallel)
    ↓
SpawnParallelTool::execute() [tools/spawn_parallel.rs]
    ↓
SubagentTracker::new() + event_sender()
    ↓
SubagentManager::task(id, prompt) → SubagentTaskBuilder [subagents/manager.rs]
    ↓
SubagentTaskBuilder::with_streaming().with_system_prompt()...
    ↓
SubagentTaskBuilder::spawn() [subagents/manager.rs]
    ↓
tokio::spawn(async { ... })
    ↓
AgentSession::with_pricing() → AgentContext::Stateless
    ↓
process_direct_streaming()
    ↓
Result → mpsc::channel → SubagentTracker
```

---

## 7. AgentContext 枚举详解

`AgentContext` 枚举抽象了状态管理，消除了核心循环中的 `Option<T>` 检查。

```rust
pub enum AgentContext {
    Persistent(PersistentContext),  // 主 Agent，完整事件溯源
    Stateless,                      // 子 Agent，无持久化
}
```

### 变体对比

| 方法 | PersistentContext | Stateless |
|------|------------------|------------------|
| `load_session` | 从 SQLite 加载 | 内存创建 |
| `save_message` | 持久化到 SQLite | No-op |
| `load_summary` | 从 SQLite 加载 | 返回 None |
| `compress_context` | 同步 LLM 摘要 | No-op |
| `recall_history` | 语义搜索 | 返回空 Vec |
| `is_persistent` | true | false |

---

## 8. Hook 系统架构

Pipeline Hook 系统提供五个执行点的扩展机制：

```rust
pub enum HookPoint {
    BeforeRequest,    // 顺序执行，可修改/中止
    AfterHistory,     // 顺序执行，可修改
    BeforeLLM,        // 顺序执行，最后修改机会
    AfterToolCall,    // 并行执行，只读
    AfterResponse,    // 并行执行，只读
}
```

### 内置 Hooks

| Hook | 类型 | 职责 |
|------|------|------|
| `ExternalShellHook` | BeforeRequest/AfterResponse | 外部 Shell 脚本扩展 |
| `HistoryRecallHook` | AfterHistory | 语义历史召回 |
| `VaultHook` | BeforeLLM | Vault 占位符注入 |

---

## 9. 上下文压缩机制

`ContextCompactor` 提供同步的上下文压缩：

```rust
pub struct ContextCompactor {
    provider: Arc<dyn LlmProvider>,
    event_store: Arc<EventStore>,
    model: String,
    token_budget: usize,
    compaction_threshold: f32,
    summarization_prompt: String,
}

impl ContextCompactor {
    /// 同步压缩（在 finalize_response 中调用）
    pub async fn compress(&self, session_key: &str, messages: &[SessionMessage]) -> Result<()>;

    /// 加载已有摘要
    pub async fn load_summary(&self, session_key: &str) -> Option<String>;

    /// 语义历史召回
    pub async fn recall_history(&self, session_key: &str, embedding: &[f32], top_k: usize)
        -> Vec<(String, f32)>;
}
```

压缩流程：
1. `process_history()` 识别被驱逐的消息
2. `finalize_response()` 中同步调用 `compress_context()`
3. `ContextCompactor` 执行 LLM 摘要
4. 摘要存储到 `session_summaries` 表

---

## 10. SubagentManager API

SubagentManager 提供 Builder 模式的任务创建 API：

```rust
// Builder 模式（推荐）
let task_id = manager
    .task("sub-1", "执行任务")
    .with_system_prompt("自定义提示词".to_string())
    .with_streaming(event_tx)
    .with_cancellation_token(token)
    .spawn(result_tx)
    .await?;

// 传统 API 仍可用
manager.submit(prompt, channel, chat_id)?;
manager.submit_and_wait(prompt, system_prompt, channel, chat_id).await?;
```

---

## 11. 文件索引

| 文件 | 职责 | 关键结构 |
|------|------|---------|
| `session/mod.rs` | 会话管理核心 | `AgentSession`, `AgentResponse`, `FinalizeContext` |
| `session/config.rs` | Agent 配置 | `AgentConfig`, `AgentConfigExt` |
| `session/context.rs` | 上下文管理 | `AgentContext` 枚举, `PersistentContext` |
| `session/compactor.rs` | 上下文压缩 | `ContextCompactor` |
| `session/memory.rs` | 记忆管理 | `MemoryManager`, `MemoryContext`, `MemoryProvider` |
| `session/prompt.rs` | 提示加载 | `load_system_prompt()`, `load_skills_context()` |
| `kernel/mod.rs` | 纯函数执行入口 | `execute()`, `execute_streaming()` |
| `kernel/executor.rs` | 核心执行引擎 | `AgentExecutor`, `ToolExecutor`, `ExecutionResult` |
| `kernel/context.rs` | 运行时上下文 | `RuntimeContext`, `KernelConfig` |
| `kernel/stream.rs` | 流事件 | `StreamEvent`, `BufferedEvents` |
| `subagents/manager.rs` | 子代理管理 | `SubagentManager`, `SubagentTaskBuilder` |
| `subagents/tracker.rs` | 并行追踪 | `SubagentTracker`, `SubagentEvent`, `SubagentResult` |
| `subagents/runner.rs` | 子代理运行 | `run_subagent()`, `ModelResolver` |
| `tools/spawn_parallel.rs` | 并行工具 | `SpawnParallelTool` |
