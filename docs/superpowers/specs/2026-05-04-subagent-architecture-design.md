# Subagent 派发架构重设计：Role + Budget

**日期**: 2026-05-04
**状态**: Draft（待用户审阅）
**作者**: Claude (会话设计稿)

## 1. 背景与动机

### 1.1 问题陈述

用户提出两条规则：

1. **只有主 agent 能 spawn**——子 agent（subagent）不可再派发新的 subagent。
2. **Spawn 必须有一个全局 threshold**——所有 spawn 路径共享同一个并发上限。

### 1.2 现状分析

当前代码用三处独立机制表达"主/子可否 spawn"，无单一真理之源（single source of truth）：

| 防御层 | 文件 | 机制 |
|---|---|---|
| ToolRegistry | 主、子共享同一份 `Arc<ToolRegistry>` | LLM 仍能看到 spawn 工具 |
| RuntimeContext | `subagents/manager.rs:149` 显式 `spawner: None` | 隐式约定，类型不强制 |
| NoopSpawner | `gasket-types/src/tool.rs:79` | 运行时 fallback，调用即返回 Err |

而"全局 threshold"在当前代码中**根本不是一个领域概念**：

- `SpawnParallelTool` 内部硬编码 `Semaphore::new(5)`（两处，blocking + non-blocking 分支各一）
- `SpawnTool` 自身完全无限流
- 两个工具不共享任何并发约束
- LLM 在同一轮响应里返回 `spawn × 3 + spawn_parallel(5)` 会一次起 8 个 subagent

### 1.3 设计目标

- **单一真理之源**：用一个枚举（`AgentRole`）表达"主/子"区分，消除三处冗余防御。
- **全局阈值是 first-class 类型**：用一个命名类型（`SpawnBudget`）承载并发预算，所有 spawn 路径强制经过同一闸门。
- **类型层强制约束**：Worker 类型层面就没有 `Spawner` 可调，不依赖任何 fallback。
- **可观测性**：`RuntimeContext.role` 字段让日志、metrics、调试可直接看到 agent 角色。

## 2. 设计决策记录

| 决策项 | 选择 | 理由 |
|---|---|---|
| 角色建模 | enum `AgentRole { Orchestrator, Worker }` | 显式 first-class 概念，替代 `Option<Spawner>` 隐式 boolean |
| 阈值建模 | 命名类型 `SpawnBudget` 包裹 `Arc<Semaphore>` | 让"全局 threshold"在领域词典里有名字 |
| 阈值持有者 | Orchestrator session（注入到 `SimpleSpawner`） | session-scoped；将来扩展（每频道预算等）有去处 |
| Worker 工具集 | Spawner 构造时**预构建一次**，不每次 spawn 时 clone+filter | 性能 + 简洁 |
| Worker 的 Spawner 字段 | `None`，类型层面就没有 | 比 NoopSpawner 更彻底：没有字段可调，无法绕过 |
| `RuntimeContext.role` | 显式新增字段 | 可观测性优先；不依赖"看 spawner 是否 None"反推 |
| 类型放置 | `gasket-types` crate | 与 `SubagentSpawner` trait 同 crate，types 是领域词典 |
| 配置项 | `tools.spawn.max_concurrency`（默认 1） | 与既有 `tools.web` / `tools.exec` 嵌套结构一致 |
| 删除 NoopSpawner | 是 | 类型 + 工具注册两层都堵住，第三层冗余 |
| 删除 SpawnParallelTool 内部 Semaphore | 是 | 限流统一上移到 `SpawnBudget` |
| 保留 `SpawnParallelTool` | 是 | 仍是有效工具；只是受全局闸门约束（声明式多任务比 LLM 自己循环 spawn 更省 token） |

## 3. 架构

```
┌──────────────────────────────────────────────────────────────────┐
│ Orchestrator Session                                             │
│                                                                  │
│   RuntimeContext {                                               │
│     role:    AgentRole::Orchestrator,                            │
│     tools:   Arc<ToolRegistry>   ← 主 agent 工具集，含 spawn / spawn_parallel │
│     spawner: Some(Arc<SimpleSpawner>),                           │
│     ...                                                          │
│   }                                                              │
│                                                                  │
│   SimpleSpawner {                                                │
│     provider:        Arc<dyn LlmProvider>,                       │
│     worker_tools:    Arc<ToolRegistry>  ← 预构建的 Worker 工具集，无 spawn │
│     workspace:       PathBuf,                                    │
│     budget:          SpawnBudget,        ← 全局 threshold 在此 │
│     token_tracker:   Option<...>,                                │
│     model_resolver:  Option<Arc<dyn ModelResolver>>,             │
│   }                                                              │
└────────────────────────────────┬─────────────────────────────────┘
                                 │ spawn(task) / spawn_with_stream(task)
                                 │   1. permit = budget.acquire().await   ← 全局闸门
                                 │   2. 构造 Worker RuntimeContext:
                                 │        role:    AgentRole::Worker
                                 │        tools:   worker_tools.clone()
                                 │        spawner: None  ← 不是 NoopSpawner
                                 │   3. tokio::spawn(work)
                                 │   4. permit move 进 task，task drop 时归还
                                 ▼
┌──────────────────────────────────────────────────────────────────┐
│ Worker Session                                                   │
│                                                                  │
│   RuntimeContext {                                               │
│     role:    AgentRole::Worker,                                  │
│     tools:   Arc<ToolRegistry>  ← Worker 工具集（无 spawn 工具）  │
│     spawner: None,                                               │
│     ...                                                          │
│   }                                                              │
│                                                                  │
│   → LLM 工具列表里没有 spawn / spawn_parallel                    │
│   → 即使越权访问，spawner 字段是 None，没有对象可调               │
└──────────────────────────────────────────────────────────────────┘
```

## 4. 类型与 API

### 4.1 新增类型

#### 4.1.1 `gasket-types/src/agent.rs`（新文件）

```rust
//! Agent role classification.

/// 区分一个 RuntimeContext 是 Orchestrator（主 agent）还是 Worker（subagent）。
///
/// Orchestrator 持有 `SubagentSpawner`，可派发 Worker；Worker 不持有 spawner，
/// 工具注册阶段也不会拿到 `spawn` / `spawn_parallel` 工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Worker,
}

impl AgentRole {
    /// Orchestrator 才能派发 Worker。
    pub fn can_spawn(&self) -> bool {
        matches!(self, AgentRole::Orchestrator)
    }
}
```

`gasket-types/src/lib.rs` 加 `pub mod agent; pub use agent::AgentRole;`

#### 4.1.2 `gasket-types/src/spawn_budget.rs`（新文件）

```rust
//! Global concurrency budget for subagent spawning.

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Orchestrator session 范围内、对 Worker 并发数的预算。
///
/// 所有 spawn 路径（`spawn` / `spawn_parallel`、未来可能的其他派发工具）必须先
/// `acquire()` 拿到 permit 才能创建 Worker。permit 跟着 Worker 的 tokio task 走，
/// task 退出时 permit 自动归还，从而限制**并发数**而非"启动速率"。
#[derive(Clone, Debug)]
pub struct SpawnBudget {
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

impl SpawnBudget {
    pub fn new(max_concurrency: usize) -> Self {
        let n = max_concurrency.max(1); // 至少 1，避免配错导致死锁
        Self {
            semaphore: Arc::new(Semaphore::new(n)),
            max_concurrency: n,
        }
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// 拿一个 permit；返回的 `OwnedSemaphorePermit` 持续到 drop 时归还。
    /// 调用者通常会 move 进 spawn 出去的 tokio task。
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        // Semaphore 在 SpawnBudget 生命周期内不会被关闭，unwrap 安全。
        self.semaphore.clone().acquire_owned().await.expect("SpawnBudget semaphore closed")
    }
}
```

### 4.2 改造类型

#### 4.2.1 `RuntimeContext` 加 `role` 字段

文件：`gasket/engine/src/kernel/context.rs`

```rust
pub struct RuntimeContext {
    pub provider: Arc<dyn LlmProvider>,
    pub tools: Arc<ToolRegistry>,
    pub config: KernelConfig,
    pub role: AgentRole,                                       // ★ 新增
    pub spawner: Option<Arc<dyn SubagentSpawner>>,             // 仅 role==Orchestrator 时为 Some
    pub token_tracker: Option<Arc<TokenTracker>>,
    // ... 其余不变
}
```

`new(...)` 默认 `role = Orchestrator`（兼容现有调用点）；Worker 路径显式传 `Worker`。

**类型不变量**（约定，非编译期强制）：
- `role == Worker` ⟹ `spawner == None`
- `role == Orchestrator` ⟹ `spawner` 可能是 `Some`（CLI 模式可能仍是 None）

约束执行方式：在 `RuntimeContext::new_worker(...)` 便利构造器内部强制 `spawner: None`；其他构造点（如 `new_orchestrator`）才允许传入 `Some`。约定通过 API 设计而非 enum 类型参数化来落地，避免侵入式泛型扩散。

#### 4.2.2 `SimpleSpawner` 改造

文件：`gasket/engine/src/subagents/manager.rs`

```rust
pub struct SimpleSpawner {
    provider: Arc<dyn LlmProvider>,
    worker_tools: Arc<ToolRegistry>,        // ★ 改名 + 预构建（原 `tools` 字段）
    workspace: std::path::PathBuf,
    budget: SpawnBudget,                    // ★ 新增
    token_tracker: Option<Arc<TokenTracker>>,
    model_resolver: Option<Arc<dyn ModelResolver>>,
}

impl SimpleSpawner {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        worker_tools: Arc<ToolRegistry>,    // 调用方负责构建 worker 版 registry
        workspace: PathBuf,
        budget: SpawnBudget,                // 调用方负责按 config 构建
    ) -> Self { ... }
}
```

`spawn` / `spawn_with_stream` 实现增加：

```rust
async fn spawn_with_stream(...) -> Result<...> {
    let permit = self.budget.acquire().await;
    // ... 构造 task_spec、spawn task，permit move 进 task
    tokio::spawn(async move {
        let _permit = permit; // RAII：task 结束时归还
        spawn_subagent(...);
    });
    ...
}
```

### 4.3 ToolRegistry 构造按 Role 分流

文件：`gasket/engine/src/tools/builder.rs` 与 `provider.rs`

`build_tool_registry` 函数（或新增 `build_for_role`）接受 `role: AgentRole`：
- `role == Orchestrator`：注册全部工具（含 `SpawnTool`、`SpawnParallelTool`）
- `role == Worker`：`CoreToolProvider::register_tools` 内跳过 `SpawnTool` 和 `SpawnParallelTool` 注册

具体实现：`CoreToolProvider` 加 `role` 字段，`register_tools` 末尾的 spawn 工具注册改为 `if self.role.can_spawn() { ... }`。

### 4.4 Spawn 工具内部简化

文件：`gasket/engine/src/tools/spawn_parallel.rs`

- 删除 `Semaphore::new(5)`（两处：non-blocking 分支 line 173-183、blocking 分支 line 248-257）。
- `SpawnParallelTool` 退化为"循环调 `spawner.spawn_with_stream`"——并发限制由 `SpawnBudget` 在 `SimpleSpawner` 层统一施加。
- `SpawnTool` 无须改动；它已经只调一次 `spawner.spawn_with_stream`。

### 4.5 删除 NoopSpawner

文件：`gasket/types/src/tool.rs`

- 删除 `NoopSpawner` struct 及其 `SubagentSpawner` impl。
- `ToolContext` 仍需在某些场景（CLI 默认、单元测试）有"无法 spawn"的状态——改为 `spawner: Option<Arc<dyn SubagentSpawner>>`（与 `RuntimeContext` 对齐），调用方按 role 决定是否传入。
- `ToolContext::default()` 中 `spawner: NoopSpawner` 改为 `spawner: None`。
- `SpawnTool::execute` 入口检查 `ctx.spawner.is_none()` → 返回明确错误。这条路径在正常流程下不会触达（Worker 不会注册 spawn 工具，因此 LLM 不会发起此调用）；保留检查作为类型健全性兜底（defense-in-depth），防御未来重构破坏不变量。

> **注**：把 `ToolContext.spawner` 从 `Arc<dyn>` 改成 `Option<Arc<dyn>>` 是为了删除 NoopSpawner 必须做的连锁改动。所有现存的 `ctx.spawner` 调用点（`spawn.rs:83`、`spawn_parallel.rs:120`）需要改成 `ctx.spawner.as_ref().ok_or(...)`。

## 5. 配置

### 5.1 配置 schema

文件：`gasket/engine/src/config/tools.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub web: WebToolsConfig,
    #[serde(default)]
    pub exec: ExecToolConfig,
    #[serde(default)]
    pub spawn: SpawnToolConfig,        // ★ 新增
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnToolConfig {
    /// 全局并发上限。所有 spawn 路径共享。最小值 1。
    #[serde(default = "default_spawn_max_concurrency")]
    pub max_concurrency: usize,
}

impl Default for SpawnToolConfig {
    fn default() -> Self {
        Self { max_concurrency: 1 }
    }
}

fn default_spawn_max_concurrency() -> usize { 1 }
```

### 5.2 `config.example.yaml`

```yaml
tools:
  spawn:
    # 全局 subagent 并发上限。默认 1：主 agent 同一时刻只能跑一个 subagent。
    # 调大允许有限并行；spawn 和 spawn_parallel 共享这个阈值。
    max_concurrency: 1
```

### 5.3 配置贯通到 SimpleSpawner

CLI 创建 `SimpleSpawner` 处（待实现时定位具体文件）：

```rust
let budget = SpawnBudget::new(config.tools.spawn.max_concurrency);
let worker_tools = Arc::new(build_tool_registry(&config, workspace, AgentRole::Worker, ...));
let spawner = SimpleSpawner::new(provider, worker_tools, workspace, budget);
```

## 6. 行为契约（验收标准）

| # | 契约 | 验证方式 |
|---|---|---|
| C1 | Worker 的 ToolRegistry 不含 `spawn` 和 `spawn_parallel` | 单元测试：`build_tool_registry(_, AgentRole::Worker)` 后 `registry.get("spawn").is_none()` 为真 |
| C2 | Worker 的 RuntimeContext.spawner 为 None | 单元测试：spawn 一个 worker，断言其 RuntimeContext.spawner.is_none() |
| C3 | Worker 的 RuntimeContext.role 为 Worker | 同上，断言 role == AgentRole::Worker |
| C4 | `max_concurrency=1` 时 3 个并发 spawn 严格串行 | 集成测试：mock 一个慢 spawn（sleep 200ms），并发发 3 个，总耗时 ≥ 600ms |
| C5 | `max_concurrency=2` 时 `spawn_parallel(["A","B","C"])` 任意时刻 inflight ≤ 2 | 单元测试：用计数器记录 inflight，断言峰值 == 2 |
| C6 | 配置缺省时 max_concurrency == 1 | YAML 默认值测试 |
| C7 | NoopSpawner 完全删除 | 编译期：`grep NoopSpawner` 无结果 |
| C8 | 现有 spawn 端到端流程不退化 | 跑 `cargo test --workspace` 全绿 |

## 7. 显式排除（Out of Scope）

为防止范围漂移，以下**不在本次范围内**，未来如有需求另起 spec：

- ❌ Worker 内嵌套 Worker（"分级 spawn"）
- ❌ 每频道 / 每用户独立预算
- ❌ 优先级队列（高优 spawn 抢占）
- ❌ Spawn 拒绝策略（满载时返回 Err 而非排队）
- ❌ Agent 通用能力系统（`Set<Capability>` 风格）
- ❌ 改 `SubagentSpawner` trait 签名
- ❌ 改 web 前端、事件流、token tracker
- ❌ 改 `RuntimeContext` 除 `role` 之外的字段

## 8. 迁移与兼容

### 8.1 调用方迁移清单

| 调用方 | 改动 |
|---|---|
| `RuntimeContext::new` | 加 `role` 参数；提供 `new_orchestrator(...)` / `new_worker(...)` 便利构造器 |
| `SimpleSpawner::new` | 签名变化：去掉 `tools` 参数，新增 `worker_tools` 和 `budget` |
| CLI/Gateway 启动代码 | 读 config.tools.spawn.max_concurrency；构建 worker_tools；构建 SpawnBudget |
| 测试代码（`MockSpawner` 三处） | 不影响（trait 未变） |
| `ToolContext::default()` 等 | spawner 字段变 Option，默认 None |
| `spawn.rs:83` / `spawn_parallel.rs:120` 取 spawner | `as_ref().ok_or(ToolError::ExecutionError(...))` |

### 8.2 兼容性

- **配置层**：用户已有的 `~/.gasket/config.yaml` 不写 `tools.spawn` 段也能跑（默认 max_concurrency=1）。**行为变化**：先前隐式可同时起 5 个并行，现在默认 1。**这是预期行为变化**，需要在 release notes 中标注。
- **运行时**：trait `SubagentSpawner` 接口完全不变，外部实现者（如插件 `MockSpawner`）零修改。

## 9. 实施步骤建议（详细 plan 由 writing-plans 输出）

1. 新增 `AgentRole` + `SpawnBudget` 类型（`gasket-types`）
2. `RuntimeContext` 加 `role` 字段；提供便利构造器；更新所有构造点
3. `ToolRegistry` 构造按 role 分流（`CoreToolProvider` 改造）
4. `SimpleSpawner` 重构字段；`spawn_with_stream` / `spawn` 调 `budget.acquire`
5. 删除 `SpawnParallelTool` 内部 `Semaphore::new(5)`
6. 删除 `NoopSpawner`；`ToolContext.spawner` 改 `Option`
7. 加 `SpawnToolConfig`；CLI 透传配置
8. 加测试（C1–C7）
9. 更新 `config.example.yaml` 和 release notes

每步独立可编译可测试。

## 10. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| 默认 max_concurrency=1 可能让既有用户觉得"变慢" | 用户体验 | release notes 显式说明；用户可改回 N |
| `ToolContext.spawner` 变 Option 触发广泛改动 | 编译广播 | 编译器会标记所有点；改动机械化 |
| `RuntimeContext` 加字段触发广泛改动 | 编译广播 | 提供 `new_orchestrator` / `new_worker` 默认构造，减少改动点 |
| Semaphore 死锁（permit 未归还） | 运行时阻塞 | RAII：permit 持有在 task 闭包，drop 时自动归还；test C4 覆盖 |

## 11. 备选方案（已淘汰）

| 方案 | 淘汰原因 |
|---|---|
| 原方案 D（仅过滤工具 + SimpleSpawner 内私有 Mutex） | 贴膏药，三处独立防御没收敛到单一概念 |
| 基于 trait 泛型的静态角色（`Agent<Orchestrator>`） | 侵入太大，需重写大量 trait bound；YAGNI |
| 通用 Capability 系统（`Set<Capability>`） | 抽象太早；当前只有一个维度 |
| 在 `RuntimeContext.spawner` 字段保持 `Arc<dyn SubagentSpawner>` + NoopSpawner | 维持现有"运行时拒绝"反模式 |
