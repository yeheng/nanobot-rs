# Engine Runtime Kernel 重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 engine 的 `AgentLoop` god struct 拆分为纯函数内核 `kernel::execute()` + 有状态会话层 `AgentSession`

**Architecture:** 四层模型 — 扩展点 (Layer 0) → 内核 (Layer 1) → 会话 (Layer 2) → 应用 (Layer 3)。内核是零状态纯函数，会话层管理生命周期，应用层组装依赖。

**Tech Stack:** Rust, tokio, thiserror, gasket-providers, gasket-storage

**Spec:** `docs/superpowers/specs/2026-04-10-engine-runtime-kernel-design.md`

**Strategy:** 新模块与旧 `agent/` 并行存在，逐步切换调用方，最后删除旧代码。每一步 `cargo build` + `cargo test` 必须通过。

---

## Task 1: 创建 kernel 目录骨架 + 类型定义

**Files:**
- Create: `engine/src/kernel/mod.rs`
- Create: `engine/src/kernel/context.rs`
- Create: `engine/src/kernel/error.rs`

- [ ] **Step 1: 创建 kernel 目录和 context.rs**

```rust
// engine/src/kernel/context.rs
//! Kernel context: dependencies needed for the pure LLM execution loop.

use std::sync::Arc;

use crate::token_tracker::TokenTracker;
use crate::tools::{SubagentSpawner, ToolRegistry};
use gasket_providers::LlmProvider;

/// Everything the kernel needs to execute one LLM request.
/// Passed by reference to `kernel::execute()` — no ownership.
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub spawner: Option<Arc<dyn SubagentSpawner>>,
    pub token_tracker: Option<Arc<TokenTracker>>,
}

/// Minimal config for the LLM iteration loop.
/// `#[non_exhaustive]` prevents external crates from adding fields.
#[non_exhaustive]
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
}

impl KernelConfig {
    pub fn new(model: String) -> Self {
        Self {
            model,
            max_iterations: 20,
            temperature: 1.0,
            max_tokens: 65536,
            max_tool_result_chars: 8000,
            thinking_enabled: false,
        }
    }

    pub fn with_max_iterations(mut self, v: u32) -> Self { self.max_iterations = v; self }
    pub fn with_temperature(mut self, v: f32) -> Self { self.temperature = v; self }
    pub fn with_max_tokens(mut self, v: u32) -> Self { self.max_tokens = v; self }
    pub fn with_thinking(mut self, v: bool) -> Self { self.thinking_enabled = v; self }
}
```

- [ ] **Step 2: 创建 kernel/error.rs**

```rust
// engine/src/kernel/error.rs
//! Kernel-specific errors.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KernelError {
    #[error("Provider request failed: {0}")]
    Provider(String),
    #[error("Max iterations ({0}) reached")]
    MaxIterations(u32),
    #[error("Tool execution failed: {0}")]
    ToolExecution(String),
}
```

- [ ] **Step 3: 创建 kernel/mod.rs (空骨架)**

```rust
// engine/src/kernel/mod.rs
//! Pure-function kernel: the LLM execution loop with zero side effects.
//!
//! The kernel knows nothing about sessions, persistence, hooks, or memory.
//! It takes messages, calls the LLM, dispatches tools, and returns a result.

pub mod context;
pub mod error;
// pub mod executor;  // Added in Task 2
// pub mod stream;    // Added in Task 3

pub use context::{KernelConfig, RuntimeContext};
pub use error::KernelError;
```

- [ ] **Step 4: 在 lib.rs 中注册 kernel 模块**

在 `engine/src/lib.rs` 的 `pub mod agent;` 之后添加：

```rust
pub mod kernel;
```

- [ ] **Step 5: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 6: Commit**

```bash
git add engine/src/kernel/ engine/src/lib.rs
git commit -m "feat(engine): add kernel module skeleton with RuntimeContext and KernelConfig"
```

---

## Task 2: 提取 AgentExecutor 到 kernel/executor.rs

**Files:**
- Create: `engine/src/kernel/executor.rs`
- Modify: `engine/src/kernel/mod.rs` (添加 executor 模块 + pub execute 函数)

**关键**: 从 `engine/src/agent/execution/executor.rs` 提取 `ToolExecutor`, `RequestHandler`, `AgentExecutor`, `ExecutionResult`, `ExecutorOptions` 及其测试。不是移动，而是复制后调整导入路径。旧文件保留不动。

- [ ] **Step 1: 创建 kernel/executor.rs**

从 `agent/execution/executor.rs` 复制以下内容，修改导入路径：
- `ToolExecutor` (executor.rs:52-128)
- `RequestHandler` (executor.rs:134-248)
- `AgentExecutor` (executor.rs:358-798)
- `ExecutionResult`, `ExecutorOptions`, `ExecutionState`, `IterationOutcome` (executor.rs:256-356)
- 测试 (executor.rs:800-993)

导入路径变更：
```rust
// 旧的
use crate::agent::core::AgentConfig;
use crate::agent::streaming::stream::{self, StreamEvent};
use crate::error::AgentError;
// 新的
use super::context::KernelConfig;
use super::error::KernelError;
use super::StreamEvent; // Task 3 adds this
```

将 `&'a AgentConfig` 引用替换为 `&'a KernelConfig`。
将 `AgentError` 替换为 `KernelError`。

- [ ] **Step 2: 在 mod.rs 中添加 execute() 和 execute_streaming() 入口函数**

```rust
// engine/src/kernel/mod.rs (追加)

pub mod executor;
pub mod stream; // Task 3 adds this

pub use executor::{ExecutionResult, ExecutorOptions};

use executor::AgentExecutor;
use gasket_providers::ChatMessage;
use tokio::sync::mpsc;

/// Pure function: execute LLM conversation loop.
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError> {
    let exec = AgentExecutor::new(
        ctx.provider.clone(),
        ctx.tools.clone(),
        &ctx.config,
    );
    exec.execute_with_options(
        messages,
        &ExecutorOptions::new()
            .maybe_with_spawner(ctx.spawner.clone())
            .maybe_with_token_tracker(ctx.token_tracker.clone()),
    ).await
}

/// Pure function: streaming LLM conversation loop.
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError> {
    let exec = AgentExecutor::new(
        ctx.provider.clone(),
        ctx.tools.clone(),
        &ctx.config,
    );
    exec.execute_stream_with_options(
        messages,
        event_tx,
        &ExecutorOptions::new()
            .maybe_with_spawner(ctx.spawner.clone())
            .maybe_with_token_tracker(ctx.token_tracker.clone()),
    ).await
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS (可能需要先完成 Task 3 的 stream 模块)

- [ ] **Step 4: Commit**

```bash
git add engine/src/kernel/
git commit -m "feat(engine): extract AgentExecutor into kernel::executor"
```

---

## Task 3: 提取 StreamEvent 到 kernel/stream.rs

**Files:**
- Create: `engine/src/kernel/stream.rs`
- Modify: `engine/src/kernel/mod.rs` (更新 stream 模块引用)

- [ ] **Step 1: 创建 kernel/stream.rs**

从 `agent/streaming/stream.rs` 复制以下内容（不改功能，只改路径）：
- `StreamEvent` enum (stream.rs:16-44)
- `ToolCallAccumulator` (stream.rs:51-109)
- `StreamAccumulator` (stream.rs:120-172)
- `stream_events()` 函数 (stream.rs:181-318)
- `collect_stream_response()` 函数 (stream.rs:324-335)
- `BufferedEvents` (stream.rs:437-478)
- 所有测试 (stream.rs:337-537)

- [ ] **Step 2: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 3: 运行 kernel 测试**

Run: `cargo test --package gasket-engine -- kernel::`
Expected: ALL PASS (包括从 stream.rs 和 executor.rs 复制过来的测试)

- [ ] **Step 4: Commit**

```bash
git add engine/src/kernel/
git commit -m "feat(engine): extract StreamEvent into kernel::stream"
```

---

## Task 4: 创建 session 模块 — AgentSession 骨架

**Files:**
- Create: `engine/src/session/mod.rs`
- Create: `engine/src/session/context.rs`
- Create: `engine/src/session/config.rs`

- [ ] **Step 1: 创建 session/context.rs — 移动 AgentContext**

从 `agent/core/context.rs` 复制 `AgentContext` enum 和 `PersistentContext` struct。
删除 deprecated 方法 (`get_history`, `recall_history`, `load_latest_summary`)。
保留：`persistent()`, `is_persistent()`, `load_summary_with_watermark`, `get_events_after_watermark`, `load_session`, `save_event`, `clear_session`。

导入路径变更：
```rust
// 旧的
use crate::agent::history::coordinator::HistoryCoordinator;
// 新的
use super::history::coordinator::HistoryCoordinator; // Task 5
```

- [ ] **Step 2: 创建 session/config.rs — 拆分 AgentConfig**

```rust
// engine/src/session/config.rs
//! Session configuration (extends KernelConfig with session-specific settings).

/// Full agent configuration for session management.
/// This extends KernelConfig with session-specific settings.
#[derive(Clone)]
pub struct AgentConfig {
    // Kernel fields (duplicated for convenience, extracted to KernelConfig at call time)
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
    // Session-specific fields
    pub streaming: bool,
    pub memory_window: usize,
    pub subagent_timeout_secs: u64,
    pub session_idle_timeout_secs: u64,
    pub summarization_prompt: Option<String>,
    pub embedding_config: Option<crate::config::EmbeddingConfig>,
}

impl AgentConfig {
    /// Extract the kernel-relevant fields into a KernelConfig.
    pub fn to_kernel_config(&self) -> crate::kernel::KernelConfig {
        crate::kernel::KernelConfig::new(self.model.clone())
            .with_max_iterations(self.max_iterations)
            .with_temperature(self.temperature)
            .with_max_tokens(self.max_tokens)
            .with_thinking(self.thinking_enabled)
    }
}
```

- [ ] **Step 3: 创建 session/mod.rs — AgentSession**

```rust
// engine/src/session/mod.rs
//! Session management layer — wraps the kernel with stateful lifecycle.

pub mod config;
pub mod context;

pub use config::AgentConfig;
pub use context::{AgentContext, PersistentContext};
```

- [ ] **Step 4: 在 lib.rs 中注册 session 模块**

在 `engine/src/lib.rs` 的 `pub mod kernel;` 之后添加：

```rust
pub mod session;
```

- [ ] **Step 5: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 6: Commit**

```bash
git add engine/src/session/ engine/src/lib.rs
git commit -m "feat(engine): add session module with AgentSession skeleton"
```

---

## Task 5: 迁移 session 子模块

**Files:**
- Move: `agent/history/` → `session/history/`
- Move: `agent/memory/compactor.rs` → `session/compactor.rs`
- Move: `agent/memory/manager.rs` → `session/memory.rs`
- Move: `agent/memory/store.rs` → `session/store.rs`
- Move: `agent/execution/prompt.rs` → `session/prompt.rs`

- [ ] **Step 1: 迁移 history/ 目录**

将 `engine/src/agent/history/` 整个目录复制到 `engine/src/session/history/`。
更新内部导入路径：
- `use crate::agent::*` → `use crate::session::*`

- [ ] **Step 2: 迁移 memory/ 子模块**

从 `engine/src/agent/memory/` 复制：
- `compactor.rs` → `session/compactor.rs`
- `manager.rs` → `session/memory.rs`
- `store.rs` → `session/store.rs`

- [ ] **Step 3: 迁移 prompt 加载**

从 `engine/src/agent/execution/prompt.rs` 复制到 `session/prompt.rs`。

- [ ] **Step 4: 更新 session/mod.rs 注册子模块**

```rust
pub mod compactor;
pub mod config;
pub mod context;
pub mod history;
pub mod memory;
pub mod prompt;
pub mod store;
```

- [ ] **Step 5: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS (旧的 agent/ 仍然存在，并行)

- [ ] **Step 6: Commit**

```bash
git add engine/src/session/
git commit -m "feat(engine): migrate history, memory, prompt into session module"
```

---

## Task 6: 实现 AgentSession.process() — 核心会话循环

**Files:**
- Modify: `engine/src/session/mod.rs` (添加 AgentSession struct + process 方法)

- [ ] **Step 1: 实现 AgentSession**

从 `agent/core/loop_.rs` 的 `AgentLoop` 提取会话管理逻辑到 `AgentSession`：

```rust
// engine/src/session/mod.rs — 追加 AgentSession

use std::path::PathBuf;
use std::sync::Arc;

use super::context::AgentContext;
use super::config::AgentConfig;
use super::history::indexing::IndexingService;
use super::memory::compactor::ContextCompactor;
use super::memory::manager::MemoryManager;
use super::memory::store::MemoryStore;
use crate::kernel::{self, RuntimeContext, KernelConfig};
use crate::hooks::HookRegistry;
use crate::tools::ToolRegistry;
use crate::error::AgentError;
use gasket_types::SessionKey;

pub struct AgentSession {
    runtime_ctx: RuntimeContext,
    context: AgentContext,
    workspace: PathBuf,
    config: AgentConfig,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: gasket_storage::HistoryConfig,
    compactor: Option<Arc<ContextCompactor>>,
    memory_manager: Option<Arc<MemoryManager>>,
    indexing_service: Option<Arc<IndexingService>>,
    pricing: Option<crate::token_tracker::ModelPricing>,
    spawner: Option<Arc<dyn crate::tools::SubagentSpawner>>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}
```

核心方法从 `loop_.rs:537-782` 提取：
- `prepare_pipeline()` → 复用 `ContextBuilder`
- `process_direct()` → 调用 `kernel::execute()`
- `process_streaming()` → 调用 `kernel::execute_streaming()`
- `finalize_response()` → 从 `loop_.rs:791-882` 复制

- [ ] **Step 2: 实现 AgentSession 构造器**

从 `loop_.rs:224-320` 提取 `new()`, `with_pricing()` 等构造方法。
关键变化：构造 `RuntimeContext` 并存储。

- [ ] **Step 3: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 4: Commit**

```bash
git add engine/src/session/
git commit -m "feat(engine): implement AgentSession with kernel::execute integration"
```

---

## Task 7: 更新 bus_adapter.rs — 切换到 AgentSession

**Files:**
- Modify: `engine/src/bus_adapter.rs`

- [ ] **Step 1: 更新 EngineHandler**

```rust
// 旧的
use crate::agent::AgentLoop;
pub struct EngineHandler {
    agent_loop: Arc<AgentLoop>,
}

// 新的
use crate::session::AgentSession;
pub struct EngineHandler {
    session: Arc<AgentSession>,
}
```

更新所有方法调用：
- `self.agent_loop.process_direct(...)` → `self.session.process_direct(...)`
- `self.agent_loop.process_direct_streaming_with_channel(...)` → `self.session.process_streaming_with_channel(...)`

- [ ] **Step 2: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 3: Commit**

```bash
git add engine/src/bus_adapter.rs
git commit -m "refactor(engine): switch bus_adapter to use AgentSession"
```

---

## Task 8: 更新 subagents/manager.rs — 直接使用 kernel

**Files:**
- Modify: `engine/src/agent/subagents/manager.rs` (行 277, 587)

- [ ] **Step 1: 替换 for_subagent 调用**

在 `manager.rs:277` (spawn_subagent_task 方法内):

```rust
// 旧的
let agent = match AgentLoop::for_subagent(provider, workspace.clone(), agent_config, tools) {
    Ok(a) => a,
    Err(e) => { /* error handling */ }
};
// ... later:
let response = agent.process_direct(&system_prompt, &session_key).await;

// 新的
let kernel_config = crate::kernel::KernelConfig::new(agent_config.model.clone())
    .with_max_iterations(agent_config.max_iterations)
    .with_temperature(agent_config.temperature)
    .with_max_tokens(agent_config.max_tokens)
    .with_thinking(agent_config.thinking_enabled);
let ctx = crate::kernel::RuntimeContext {
    provider,
    tools,
    config: kernel_config,
    spawner: None,
    token_tracker: self.token_tracker.clone(),
};
// Build messages with system prompt
let messages = vec![
    gasket_providers::ChatMessage::system(&system_prompt),
    gasket_providers::ChatMessage::user(&task),
];
let response = crate::kernel::execute(&ctx, messages).await;
```

在 `manager.rs:587` (build_subagent_internal 函数):

```rust
// 旧的
let mut agent = AgentLoop::for_subagent(provider, workspace.clone(), config, tools)?;
// ... system prompt setup
Ok(agent)

// 新的: 删除此函数，逻辑内联到调用方
```

- [ ] **Step 2: 验证编译**

Run: `cargo build --package gasket-engine`
Expected: BUILD SUCCEEDS

- [ ] **Step 3: Commit**

```bash
git add engine/src/agent/subagents/
git commit -m "refactor(engine): subagents use kernel::execute directly"
```

---

## Task 9: 更新 CLI 调用方

**Files:**
- Modify: `cli/src/commands/registry.rs`
- Modify: `cli/src/commands/agent.rs` (如果直接引用 AgentLoop)
- Modify: `cli/src/commands/gateway.rs` (如果直接引用 AgentLoop)

- [ ] **Step 1: 搜索所有 AgentLoop 引用**

Run: `grep -rn "AgentLoop" cli/src/`
记录所有需要更新的位置。

- [ ] **Step 2: 更新 registry.rs**

```rust
// 旧的
use gasket_engine::agent::AgentLoop;
use gasket_engine::agent::AgentConfig;

// 新的
use gasket_engine::session::AgentSession;
use gasket_engine::session::AgentConfig;
use gasket_engine::kernel::KernelConfig;
```

更新 `build_agent_config()` 添加 `to_kernel_config()` 方法调用处。

- [ ] **Step 3: 更新 agent command**

将 `AgentLoop::new()` 或 `AgentLoop::with_pricing()` 替换为 `AgentSession::new()` 或 `AgentSession::with_pricing()`。

- [ ] **Step 4: 更新 gateway command**

同上。

- [ ] **Step 5: 验证编译和测试**

Run: `cargo build --workspace && cargo test --workspace`
Expected: BUILD SUCCEEDS, ALL TESTS PASS

- [ ] **Step 6: Commit**

```bash
git add cli/src/
git commit -m "refactor(cli): switch to AgentSession + kernel::execute"
```

---

## Task 10: 更新 lib.rs — 向后兼容别名 + 删除旧 agent/

**Files:**
- Modify: `engine/src/lib.rs`
- Delete: `engine/src/agent/` (整个目录)

- [ ] **Step 1: 更新 lib.rs 添加向后兼容别名**

```rust
// engine/src/lib.rs — 在模块声明后添加

// ── Backward-compatible type aliases ──────────────────────
pub use session::AgentSession as AgentLoop;
pub use session::AgentConfig;
```

保留旧的 re-exports 但指向新位置：
```rust
// 更新 agent 模块为空壳（或删除）
// pub mod agent; ← 删除此行

// 新的导出路径
pub use kernel;
pub use session;
```

- [ ] **Step 2: 搜索 engine 内部对 agent:: 的引用并更新**

Run: `grep -rn "use crate::agent::" engine/src/`
Run: `grep -rn "use super::" engine/src/agent/` (已删除，无需搜索)

所有 `use crate::agent::*` 替换为对应的新路径：
- `crate::agent::AgentLoop` → `crate::session::AgentSession` (或 `AgentLoop` 别名)
- `crate::agent::AgentConfig` → `crate::session::AgentConfig`
- `crate::agent::execution::*` → `crate::kernel::*`
- `crate::agent::streaming::*` → `crate::kernel::stream::*`
- `crate::agent::history::*` → `crate::session::history::*`
- `crate::agent::memory::*` → `crate::session::*`

- [ ] **Step 3: 删除 agent/ 目录**

```bash
rm -rf engine/src/agent/
```

- [ ] **Step 4: 验证编译**

Run: `cargo build --workspace`
Expected: BUILD SUCCEEDS (如果有编译错误，修复导入路径)

- [ ] **Step 5: 运行完整测试**

Run: `cargo test --workspace`
Expected: ALL TESTS PASS

- [ ] **Step 6: Commit**

```bash
git add -A engine/src/
git commit -m "refactor(engine): remove old agent/ module, switch to kernel + session"
```

---

## Task 11: 清理 deprecated 代码

**Files:**
- Modify: `engine/src/session/context.rs` (删除 deprecated 方法)
- Modify: `engine/src/bus_adapter.rs` (删除 AgentLoop 上的 MessageHandler impl)
- Modify: `engine/src/kernel/` (删除 SharedTextEmbedder 包装器，如果在 session 中也需要清理)

- [ ] **Step 1: 删除 AgentContext 的 deprecated 方法**

在 `session/context.rs` 中删除：
- `get_history()` (标记 `#[deprecated]`)
- `recall_history()` (标记 `#[deprecated]`)
- `load_latest_summary()` (标记 `#[deprecated]`)

- [ ] **Step 2: 删除 AgentLoop 上的 MessageHandler impl**

在 `loop_.rs` 被删除后，确认 `engine/src/agent/core/loop_.rs:922-998` 的 `impl MessageHandler for AgentLoop` 已不存在。保留 `bus_adapter.rs` 中的 `impl MessageHandler for EngineHandler`。

- [ ] **Step 3: 清理 SharedTextEmbedder**

如果 `session/` 中仍使用，将其从内核无关代码中移出，仅在 `session/memory.rs` 中使用。

- [ ] **Step 4: 验证编译和测试**

Run: `cargo build --workspace && cargo test --workspace`
Expected: BUILD SUCCEEDS, ALL TESTS PASS

- [ ] **Step 5: Commit**

```bash
git add -A engine/src/
git commit -m "refactor(engine): clean up deprecated methods and SharedTextEmbedder wrapper"
```

---

## Task 12: 最终验证 + 文档更新

**Files:**
- Modify: `engine/README.md` (如果有)
- Modify: `CLAUDE.md` (更新架构说明)

- [ ] **Step 1: 运行完整测试套件**

Run: `cargo test --workspace`
Expected: ALL TESTS PASS

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy --workspace`
Expected: NO WARNINGS

- [ ] **Step 3: 更新 CLAUDE.md 架构说明**

更新 Workspace Structure 和 Architecture Notes 部分，反映新的 kernel/session 分层。

- [ ] **Step 4: 更新 engine/README.md**

如果有 README，更新模块说明。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: update architecture docs for kernel/session refactoring"
```

---

## 验证清单

- [ ] `kernel::execute()` 可独立调用，无 session 依赖
- [ ] 子 agent 直接使用 `kernel::execute()`，无 `for_subagent()`
- [ ] `AgentSession.process()` 行为与旧 `AgentLoop::process_direct()` 一致
- [ ] 所有测试通过: `cargo test --workspace`
- [ ] 无 clippy 警告: `cargo clippy --workspace`
- [ ] `engine/src/kernel/` 行数 < 800 (含测试)
- [ ] `agent/` 目录已完全删除
