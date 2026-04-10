# Engine Runtime Kernel 重构设计

**日期**: 2026-04-10
**状态**: Draft
**范围**: `gasket-engine` crate

## 1. 动机

当前 `AgentLoop` 是一个 god struct：

- 15+ 字段，混合了不变的依赖（provider, tools）和变化的请求状态（session, messages）
- 1000 行核心逻辑，直接耦合 provider / tools / hooks / memory / compaction / vault / indexing
- 扩展新功能需要修改 `AgentLoop` 本身
- 子 agent 和主 agent 共享同一个 `AgentLoop` struct，通过 `for_subagent()` 跳过初始化 — 这是一个"用 `Option` 糊边界"的反模式

**目标**：将 engine 拆分为纯函数内核 + 有状态会话层，让核心 LLM 循环可独立测试和复用。

## 2. 架构

### 2.1 分层模型

```
Layer 3 (应用层):   CLI/Gateway — 配置加载、服务组装
Layer 2 (会话层):   AgentSession — 会话管理、历史压缩、内存注入
Layer 1 (内核层):   kernel::execute() — 纯 LLM 循环、工具分发、钩子管线
Layer 0 (扩展点):   Tool, PipelineHook, LlmProvider — trait 接口
```

### 2.2 内核定义

内核是两个纯函数，零自有状态：

```rust
// engine/src/kernel/context.rs

/// 内核执行所需的全部依赖 — 每次请求传入
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub hooks: Arc<HookRegistry>,
    pub config: KernelConfig,
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<TokenTracker>>,
}

/// 内核最小配置 — 只包含 LLM 循环需要的参数
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
}
```

```rust
// engine/src/kernel/mod.rs

/// 纯函数：执行 LLM 对话循环
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError>;

/// 纯函数：流式执行 LLM 对话循环
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError>;
```

**内核职责**（且仅这些）：
- 构建 LLM 请求
- 发送请求到 provider（含重试）
- 收集流式/非流式响应
- 执行工具调用（通过 ToolRegistry）
- 循环迭代直到完成或达到 max_iterations

**内核不负责**：session 持久化、event sourcing、历史压缩、vault 注入、内存召回、cron 调度。

### 2.3 会话层

`AgentSession` 封装内核，管理有状态的会话生命周期：

```rust
// engine/src/session/mod.rs

pub struct AgentSession {
    // 内核依赖（每次请求传给 kernel::execute）
    runtime_ctx: RuntimeContext,
    // 会话状态
    context: AgentContext,           // Persistent / Stateless
    system_prompt: String,
    skills_context: Option<String>,
    history_config: HistoryConfig,
    // 后台服务
    compactor: Option<Arc<ContextCompactor>>,
    memory_manager: Option<Arc<MemoryManager>>,
    indexing_service: Option<Arc<IndexingService>>,
}
```

**会话层职责**：
- 加载/保存 session 事件（EventStore）
- 构建完整 prompt（system + skills + history + vault + memory）
- 调用 `kernel::execute()` 或 `kernel::execute_streaming()`
- 后台压缩（compaction）
- 触发 BeforeRequest / AfterResponse 钩子

### 2.4 子 Agent 简化

子 agent 不需要 `AgentSession`，直接使用内核：

```rust
// 重构前
let sub = AgentLoop::for_subagent(provider, workspace, config, tools);

// 重构后
let ctx = RuntimeContext::new(provider, tools, HookRegistry::empty(), kernel_config);
let result = kernel::execute(&ctx, messages).await?;
```

消除了 `for_subagent()` 的 `Option` 糊边界反模式。

## 3. 模块重组

### 3.1 目录结构

```
engine/src/
├── kernel/              ← Layer 1: 纯函数内核
│   ├── mod.rs           # pub execute(), execute_streaming()
│   ├── executor.rs      # AgentExecutor (从 execution/executor.rs 迁移)
│   ├── context.rs       # RuntimeContext, KernelConfig
│   ├── stream.rs        # StreamEvent (从 streaming/ 迁移)
│   └── error.rs         # KernelError
│
├── session/             ← Layer 2: 有状态会话
│   ├── mod.rs           # AgentSession (替代 AgentLoop)
│   ├── context.rs       # AgentContext (Persistent/Stateless)
│   ├── config.rs        # SessionConfig + AgentConfig (完整配置)
│   ├── pipeline.rs      # ContextBuilder, prepare_pipeline
│   ├── compactor.rs     # ContextCompactor
│   ├── memory.rs        # MemoryManager
│   ├── indexing.rs      # IndexingService
│   └── history/         # HistoryCoordinator, builder
│
├── tools/               ← Layer 0: 工具扩展点 (保持不变)
├── hooks/               ← Layer 0: 钩子扩展点 (保持不变)
├── subagents/           ← 子 agent 管理 (保持不变)
├── config/              ← 应用配置 (保持不变)
├── vault/               ← 密钥管理 (保持不变)
├── skills/              ← Skills 加载 (保持不变)
├── cron/                ← 定时任务 (保持不变)
├── bus/                 ← Bus re-export (保持不变)
├── channels/            ← Channels re-export (保持不变)
├── providers/           ← Providers re-export (保持不变)
├── memory/              ← Memory re-export (保持不变)
└── lib.rs               # 门面 (更新 pub use)
```

### 3.2 消除的模块

| 消除的模块 | 去向 |
|-----------|------|
| `agent/` | 拆分为 `kernel/` + `session/` |
| `agent/core/` | `session/mod.rs` + `session/config.rs` + `session/context.rs` |
| `agent/execution/` | `kernel/executor.rs` |
| `agent/streaming/` | `kernel/stream.rs` |
| `agent/history/` | `session/history/` |
| `agent/memory/` | `session/memory.rs` + `session/compactor.rs` |
| `agent/subagents/` | 顶层 `subagents/` |

## 4. 数据流

```
用户消息 "hello"
    │
    ▼
AgentSession.process("hello", session_key)
    │
    ├─ 1. context.load_session(session_key)
    ├─ 2. context.save_event(user_event)
    ├─ 3. ContextBuilder.build()
    │     ├─ system_prompt + skills
    │     ├─ history (truncated to token_budget)
    │     ├─ vault injection (Hook: BeforeLLM)
    │     └─ memory recall (Hook: AfterHistory)
    │
    ├─ 4. kernel::execute(&runtime_ctx, msgs)
    │     ├─ RequestHandler.build_chat_request()
    │     ├─ provider.chat_stream(request)
    │     ├─ stream → collect response
    │     ├─ if tool_calls → ToolExecutor.execute_one()
    │     └─ repeat until done or max_iterations
    │
    ├─ 5. context.save_event(assistant_event)
    ├─ 6. compactor.try_compact()
    └─ 7. hooks.execute(AfterResponse)
```

**关键属性**：步骤 4 是纯函数，无副作用。步骤 1-3 和 5-7 是会话层管理。

## 5. 调用方迁移

### 5.1 CLI 侧

`cli/commands/registry.rs` 拆分配置构建：

```rust
// 当前
pub fn build_agent_config(config: &Config) -> AgentConfig

// 重构后
pub fn build_kernel_config(config: &Config) -> KernelConfig { ... }
pub fn build_session_config(config: &Config) -> SessionConfig { ... }
```

### 5.2 Bus 集成

`bus_adapter.rs` 内部从 `AgentLoop` 改为 `AgentSession`：

```rust
pub struct EngineHandler {
    session: Arc<AgentSession>,  // 替代 Arc<AgentLoop>
}
```

### 5.3 向后兼容

`lib.rs` 保留类型别名，不破坏外部消费者：

```rust
// 向后兼容
pub use session::AgentSession as AgentLoop;
pub use session::config::AgentConfig;
pub use kernel::execute as kernel_execute;
```

## 6. 清理项

在重构过程中同时清理：

1. **删除 `AgentContext` 的 deprecated 方法**：`get_history()`, `recall_history()`, `load_latest_summary()` 已标记 `#[deprecated]`，重构时直接移除
2. **消除 `SharedTextEmbedder` 包装器**：内核不持有 embedder，这个只在会话层需要的类型可以简化
3. **简化 `bus_adapter.rs` 中的重复代码**：当前 `EngineHandler` 和 `AgentLoop` 都实现了 `MessageHandler`，重构后只保留 `EngineHandler`

## 7. 风险分析

| 风险 | 缓解 |
|------|------|
| `AgentLoop` 被 5+ 处引用 | 类型别名 `pub use AgentSession as AgentLoop` |
| 测试中断 | 内核纯函数更容易测试；现有集成测试通过别名保持兼容 |
| `KernelConfig` 膨胀 | 严格 code review：只有 LLM 循环需要的参数才能进 `KernelConfig` |
| 重构范围过大 | 分阶段实施：先拆 kernel/session，再清理 deprecated |

## 8. 实施阶段

### Phase 1: 创建 kernel 模块
- 创建 `engine/src/kernel/` 目录
- 从 `agent/execution/executor.rs` 提取 `AgentExecutor` → `kernel::execute()`
- 定义 `RuntimeContext`, `KernelConfig`, `KernelError`
- 编写内核单元测试

### Phase 2: 创建 session 模块
- 创建 `engine/src/session/` 目录
- 从 `agent/core/loop_.rs` 提取 `AgentSession`
- `AgentSession.process()` 内部调用 `kernel::execute()`
- 迁移 compactor, memory, indexing, history 到 session/

### Phase 3: 更新调用方
- 更新 `bus_adapter.rs`
- 更新 `cli/commands/registry.rs`
- 更新 `subagents/` 使用 `kernel::execute()` 代替 `for_subagent()`

### Phase 4: 清理
- 删除 `agent/` 目录
- 删除 deprecated 方法
- 更新 `lib.rs` 的 pub use 和类型别名
- 运行完整测试套件

## 9. 成功标准

1. `kernel::execute()` 可独立调用，无 session 依赖
2. 子 agent 直接使用 `kernel::execute()`，无需 `for_subagent()`
3. `AgentSession` 的 `process()` 方法行为与当前 `AgentLoop::process_direct()` 完全一致
4. 所有现有测试通过（通过别名或直接更新）
5. `engine/src/kernel/` 总行数 < 500 行
