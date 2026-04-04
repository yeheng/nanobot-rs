# 历史记录模块重新设计实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 通过接口优先边界重构，将历史记录模块重构为事件溯源 + CQRS 架构

**Architecture:** 引入 HistoryCoordinator 作为 Agent Loop 的唯一历史相关入口，提取 EventStore trait 和 MemoryProvider trait 收窄职责边界，通过 MaterializationEngine + EventHandler 包装现有组件实现事件驱动物化。

**Tech Stack:** Rust (edition 2021), tokio (async runtime), sqlx (SQLite), tokio::sync::broadcast (事件发布), serde_json (序列化)

**Spec:** `docs/superpowers/specs/2026-04-04-history-redesign-design.md`

---

## 文件结构图

### 新建文件

| 文件 | 职责 |
|---|---|
| `gasket/engine/src/agent/history_coordinator.rs` | HistoryCoordinator + HistoryQuery + HistoryResult |
| `gasket/engine/src/agent/materialization.rs` | MaterializationEngine + EventHandler trait + HandlerContext + Checkpoint + CheckpointStore |
| `gasket/engine/src/agent/handlers/mod.rs` | Handler 模块导出 |
| `gasket/engine/src/agent/handlers/indexing_handler.rs` | IndexingHandler |
| `gasket/engine/src/agent/handlers/compaction_handler.rs` | CompactionHandler |
| `gasket/engine/src/agent/handlers/memory_update_handler.rs` | MemoryUpdateHandler |

### 修改文件

| 文件 | 变更 |
|---|---|
| `gasket/storage/src/event_store.rs` | 添加 broadcast channel、sequence 列、EventStore trait impl |
| `gasket/storage/src/lib.rs` | 导出 EventStore trait |
| `gasket/engine/src/agent/mod.rs` | 添加新模块 |
| `gasket/engine/src/agent/context.rs` | 内部使用 HistoryCoordinator |
| `gasket/engine/src/agent/loop_.rs` | 通过 Coordinator 调用 |
| `gasket/engine/src/agent/memory_manager.rs` | 实现 MemoryProvider trait |

---

## Phase 1: Facade 引入

> 目标：创建 HistoryCoordinator 门面，Agent Loop 行为不变

### Task 1: 定义 HistoryQuery 和 HistoryResult 类型

**Files:**
- Create: `gasket/engine/src/agent/history_coordinator.rs`

- [ ] **Step 1: 创建 history_coordinator.rs，定义查询和结果类型**

```rust
// gasket/engine/src/agent/history_coordinator.rs

use chrono::{DateTime, Utc};
use gasket_types::session_event::SessionEvent;
use gasket_storage::memory::types::{MemoryHit, MemoryQuery, MemoryContext};

/// 历史查询意图 — 唯一的查询入口类型
#[derive(Debug)]
pub enum HistoryQuery {
    /// 获取会话最近上下文（token 预算内）
    /// 路由到 ContextCompactor
    SessionContext {
        session_key: String,
        token_budget: usize,
    },
    /// 获取最新摘要
    /// 路由到 ContextCompactor::load_summary()
    LatestSummary {
        session_key: String,
    },
    /// 跨会话语义搜索
    /// 路由到 MemoryProvider::search()
    SemanticSearch {
        query: String,
        top_k: usize,
    },
    /// 三阶段记忆加载
    /// 路由到 MemoryProvider::load_for_context()
    MemoryContext {
        query: MemoryQuery,
    },
    /// 时间范围原始事件
    /// 路由到 EventStore::query()
    TimeRange {
        session_key: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

/// 历史查询结果
#[derive(Debug)]
pub enum HistoryResult {
    Context(Vec<String>),            // Compactor 返回的消息文本
    Summary(Option<String>),         // Compactor 返回的摘要
    Memories(Vec<MemoryHit>),        // MemoryProvider 返回
    MemoryContext(MemoryContext),     // MemoryProvider 返回
    Events(Vec<SessionEvent>),       // EventStore 返回
}
```

- [ ] **Step 2: 运行编译验证类型定义**

Run: `cargo build --package gasket-engine 2>&1 | head -20`
Expected: 编译错误 — mod 未注册（下一步注册）

- [ ] **Step 3: 在 agent/mod.rs 中注册模块**

在 `gasket/engine/src/agent/mod.rs` 的 `mod` 块中添加:
```rust
pub mod history_coordinator;
```

在 pub use 块中添加:
```rust
pub use history_coordinator::{HistoryQuery, HistoryResult};
```

- [ ] **Step 4: 运行编译确认通过**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: `Finished` (可能有 warning 但无 error)

- [ ] **Step 5: 提交**

```bash
git add gasket/engine/src/agent/history_coordinator.rs gasket/engine/src/agent/mod.rs
git commit -m "feat(engine): add HistoryQuery and HistoryResult types"
```

---

### Task 2: 实现 HistoryCoordinator 门面

**Files:**
- Modify: `gasket/engine/src/agent/history_coordinator.rs`

- [ ] **Step 1: 添加 HistoryCoordinator struct 和路由方法**

在 `history_coordinator.rs` 中追加（在 imports 之后）:

```rust
use std::sync::Arc;
use gasket_storage::EventStore;
use crate::agent::compactor::ContextCompactor;
use crate::agent::memory_manager::MemoryManager;

/// 历史查询协调器 — Agent Loop 的唯一历史相关接口
///
/// 薄路由层：根据查询意图路由到最优组件。
/// 不包含业务逻辑，所有计算委托给现有组件。
pub struct HistoryCoordinator {
    event_store: Arc<EventStore>,
    compactor: Arc<ContextCompactor>,
    memory: Arc<MemoryManager>,
}

impl HistoryCoordinator {
    pub fn new(
        event_store: Arc<EventStore>,
        compactor: Arc<ContextCompactor>,
        memory: Arc<MemoryManager>,
    ) -> Self {
        Self { event_store, compactor, memory }
    }

    /// 统一查询入口
    pub async fn query(&self, query: HistoryQuery) -> anyhow::Result<HistoryResult> {
        match query {
            HistoryQuery::SessionContext { session_key, token_budget } => {
                // TODO Phase 1: 直接委托给现有逻辑
                // Phase 2+: 委托给 ContextCompactor::get_context()
                todo!("Phase 1 — wire to existing process_history")
            }
            HistoryQuery::LatestSummary { session_key } => {
                todo!("Phase 1 — wire to existing load_latest_summary")
            }
            HistoryQuery::SemanticSearch { query, top_k } => {
                todo!("Phase 1 — wire to existing recall_history")
            }
            HistoryQuery::MemoryContext { query } => {
                let ctx = self.memory.load_for_context(&query).await?;
                Ok(HistoryResult::MemoryContext(ctx))
            }
            HistoryQuery::TimeRange { session_key, start, end } => {
                let events = self.event_store
                    .get_branch_history(&session_key, "main")
                    .await?;
                let filtered: Vec<_> = events
                    .into_iter()
                    .filter(|e| e.created_at >= start && e.created_at <= end)
                    .collect();
                Ok(HistoryResult::Events(filtered))
            }
        }
    }

    /// 保存事件 — 委托给 EventStore
    pub async fn save_event(
        &self,
        event: &gasket_types::session_event::SessionEvent,
    ) -> anyhow::Result<()> {
        self.event_store.append_event(event).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: 在 mod.rs 中导出 HistoryCoordinator**

在 `gasket/engine/src/agent/mod.rs` 的 `pub use` 中添加:
```rust
pub use history_coordinator::HistoryCoordinator;
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过（todo! 宏会 panic 但编译无 error）

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/history_coordinator.rs gasket/engine/src/agent/mod.rs
git commit -m "feat(engine): add HistoryCoordinator facade with routing"
```

---

### Task 3: 在 AgentContext 中注入 HistoryCoordinator

**Files:**
- Modify: `gasket/engine/src/agent/context.rs`

- [ ] **Step 1: 在 PersistentContext 中添加 coordinator 字段**

在 `context.rs` 的 `PersistentContext` struct 中添加:
```rust
pub struct PersistentContext {
    pub event_store: Arc<EventStore>,
    pub sqlite_store: Arc<gasket_storage::SqliteStore>,
    pub coordinator: Option<Arc<HistoryCoordinator>>,  // NEW — Phase 1 可选
    // ... 其余字段 ...
}
```

- [ ] **Step 2: 添加 coordinator setter 方法**

在 `PersistentContext` impl 中添加:
```rust
pub fn set_coordinator(&mut self, coordinator: Arc<HistoryCoordinator>) {
    self.coordinator = Some(coordinator);
}
```

- [ ] **Step 3: 运行编译和测试**

Run: `cargo build --package gasket-engine && cargo test --package gasket-engine 2>&1 | tail -10`
Expected: 编译通过，现有测试通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/context.rs
git commit -m "feat(engine): add optional HistoryCoordinator to PersistentContext"
```

---

### Task 4: 实现 SessionContext 和 LatestSummary 路由

**Files:**
- Modify: `gasket/engine/src/agent/history_coordinator.rs`

- [ ] **Step 1: 实现 SessionContext 路由**

替换 HistoryCoordinator 中 `SessionContext` 的 `todo!()`:

```rust
HistoryQuery::SessionContext { session_key, token_budget } => {
    // Phase 1: 委托给现有 get_branch_history + process_history
    let events = self.event_store
        .get_branch_history(&session_key, "main")
        .await?;

    // 简单 token budget 裁剪：从最近事件开始，直到超出预算
    let mut selected = Vec::new();
    let mut tokens_used = 0;
    for event in events.into_iter().rev() {
        let event_tokens = event.metadata.content_token_len;
        if tokens_used + event_tokens > token_budget {
            break;
        }
        tokens_used += event_tokens;
        selected.push(event.content);
    }
    selected.reverse();
    Ok(HistoryResult::Context(selected))
}
```

- [ ] **Step 2: 实现 LatestSummary 路由**

替换 `LatestSummary` 的 `todo!()`:

```rust
HistoryQuery::LatestSummary { session_key } => {
    let summary = self.event_store
        .get_latest_summary(&session_key, "main")
        .await?;
    Ok(HistoryResult::Summary(summary.map(|e| e.content)))
}
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/history_coordinator.rs
git commit -m "feat(engine): implement SessionContext and LatestSummary routing"
```

---

### Task 5: 实现 SemanticSearch 路由

**Files:**
- Modify: `gasket/engine/src/agent/history_coordinator.rs`

- [ ] **Step 1: 实现 SemanticSearch 路由**

替换 `SemanticSearch` 的 `todo!()`:

```rust
HistoryQuery::SemanticSearch { query, top_k } => {
    // Phase 1: 委托给 MemoryManager 的 search
    let hits = self.memory.search(&query, top_k).await?;
    Ok(HistoryResult::Memories(hits))
}
```

注意：需要确认 `MemoryManager` 是否有 `search()` 公共方法。如果没有，需要先在 `MemoryManager` 中暴露。

- [ ] **Step 2: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 3: 运行全量测试**

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: 所有现有测试通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/history_coordinator.rs
git commit -m "feat(engine): implement SemanticSearch routing via MemoryManager"
```

---

## Phase 2: Trait 提取

> 目标：从具体类型提取 EventStore trait 和 MemoryProvider trait，Coordinator 依赖 trait

### Task 6: 添加 sequence 列到 SessionEvent

**Files:**
- Modify: `gasket/types/src/session_event.rs`
- Modify: `gasket/storage/src/event_store.rs`

- [ ] **Step 1: 在 SessionEvent struct 中添加 sequence 字段**

在 `gasket/types/src/session_event.rs` 的 `SessionEvent` struct 中添加:
```rust
pub struct SessionEvent {
    // ... 现有字段 ...
    /// 单调递增序列号，用于增量同步和 checkpoint
    pub sequence: i64,
}
```

- [ ] **Step 2: 在 SessionEvent 的构造处添加默认值**

找到所有创建 `SessionEvent` 的位置，添加 `sequence: 0` 作为默认值。
使用 grep 查找:
```bash
grep -rn "SessionEvent {" gasket/ --include="*.rs"
```

每个构造处添加 `sequence: 0`（后续在 append_event 时由 EventStore 生成真实值）。

- [ ] **Step 3: 在 EventStore 的 SQL schema 中添加 sequence 列**

在 `gasket/storage/src/event_store.rs` 的建表语句中添加:
```sql
sequence INTEGER NOT NULL DEFAULT 0,
```

- [ ] **Step 4: 在 append_event 中生成 sequence 值**

在 `EventStore::append_event()` 方法中，插入前查询当前最大 sequence 并 +1:
```rust
let max_seq: i64 = sqlx::query_scalar(
    "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE session_key = ?"
)
.bind(&event.session_key)
.fetch_one(&*self.pool)
.await?;

// 插入时使用 max_seq + 1
```

- [ ] **Step 5: 在 EventRow 的 row_to_event 映射中添加 sequence**

- [ ] **Step 6: 运行编译和测试**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`
Expected: 编译通过，所有测试通过

- [ ] **Step 7: 提交**

```bash
git add gasket/types/src/session_event.rs gasket/storage/src/event_store.rs
git commit -m "feat(storage): add sequence column to SessionEvent for incremental sync"
```

---

### Task 7: 定义 EventStore trait

**Files:**
- Modify: `gasket/storage/src/event_store.rs`
- Modify: `gasket/storage/src/lib.rs`

- [ ] **Step 1: 在 event_store.rs 顶部定义 EventStore trait**

```rust
use tokio::sync::broadcast;

/// EventStore trait — 事件日志的窄接口
///
/// 职责：追加事件、查询事件、订阅事件流
/// 不包含：截断、摘要管理、embedding 生成
#[async_trait::async_trait]
pub trait EventStoreTrait: Send + Sync {
    /// 追加事件到存储
    async fn append(&self, event: &SessionEvent) -> Result<EventId, StoreError>;

    /// 按过滤条件查询事件
    async fn query_events(&self, filter: EventFilter) -> Result<Vec<SessionEvent>, StoreError>;

    /// 订阅新事件流
    fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;
}

/// 事件查询过滤条件
#[derive(Debug, Default)]
pub struct EventFilter {
    pub session_key: Option<String>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub event_types: Option<Vec<EventType>>,
    pub event_ids: Option<Vec<Uuid>>,
    pub limit: Option<usize>,
    pub branch: Option<String>,
}
```

注意：trait 命名为 `EventStoreTrait` 以避免与现有 `EventStore` struct 冲突。在 Phase 4 中可以 rename struct。

- [ ] **Step 2: 在 lib.rs 中导出 trait**

在 `gasket/storage/src/lib.rs` 的 pub use 块中添加:
```rust
pub use event_store::{EventStoreTrait, EventFilter};
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-storage 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add gasket/storage/src/event_store.rs gasket/storage/src/lib.rs
git commit -m "feat(storage): define EventStoreTrait with append, query, subscribe"
```

---

### Task 8: 为 SqliteEventStore 添加 broadcast channel 并实现 trait

**Files:**
- Modify: `gasket/storage/src/event_store.rs`

- [ ] **Step 1: 在 EventStore struct 中添加 broadcast sender**

```rust
pub struct EventStore {
    pool: SqlitePool,
    tx: broadcast::Sender<SessionEvent>,
}

impl EventStore {
    pub fn new(pool: SqlitePool) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self { pool, tx }
    }
}
```

- [ ] **Step 2: 在 append_event 末尾发送 broadcast**

在 `append_event()` 成功插入后:
```rust
// 发送通知（忽略 send 错误 — 无订阅者时正常）
let _ = self.tx.send(event.clone());
```

- [ ] **Step 3: 实现 EventStoreTrait for EventStore**

```rust
#[async_trait::async_trait]
impl EventStoreTrait for EventStore {
    async fn append(&self, event: &SessionEvent) -> Result<EventId, StoreError> {
        self.append_event(event).await?;
        Ok(event.id)
    }

    async fn query_events(&self, filter: EventFilter) -> Result<Vec<SessionEvent>, StoreError> {
        // 委托给现有方法
        let session_key = filter.session_key.unwrap_or_default();
        let branch = filter.branch.unwrap_or_else(|| "main".to_string());
        let mut events = self.get_branch_history(&session_key, &branch).await?;

        // 应用过滤
        if let Some(time_range) = filter.time_range {
            events.retain(|e| e.created_at >= time_range.0 && e.created_at <= time_range.1);
        }
        if let Some(event_types) = &filter.event_types {
            events.retain(|e| event_types.contains(&e.event_type));
        }
        if let Some(event_ids) = &filter.event_ids {
            let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
            return self.get_events_by_ids(&session_key, &ids).await;
        }
        if let Some(limit) = filter.limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.tx.subscribe()
    }
}
```

- [ ] **Step 4: 运行编译和测试**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`
Expected: 编译通过，所有测试通过

- [ ] **Step 5: 提交**

```bash
git add gasket/storage/src/event_store.rs
git commit -m "feat(storage): add broadcast channel and implement EventStoreTrait"
```

---

### Task 9: 定义 MemoryProvider trait

**Files:**
- Create: `gasket/engine/src/agent/memory_provider.rs`
- Modify: `gasket/engine/src/agent/mod.rs`

- [ ] **Step 1: 创建 memory_provider.rs**

```rust
// gasket/engine/src/agent/memory_provider.rs

use anyhow::Result;
use async_trait::async_trait;
use gasket_storage::memory::types::{
    MemoryHit, MemoryQuery, MemoryContext,
};
use gasket_types::session_event::SessionEvent;

/// MemoryProvider trait — 记忆系统的查询接口
///
/// 从 MemoryManager 提取，保持 async 签名与现有实现匹配
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// 三阶段加载（bootstrap/scenario/on-demand）
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext>;

    /// 语义搜索
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>>;

    /// 从事件中提取知识（由 MemoryUpdateHandler 调用）
    async fn update_from_event(&self, event: &SessionEvent) -> Result<()>;
}
```

- [ ] **Step 2: 在 mod.rs 中注册模块**

```rust
pub mod memory_provider;
```

- [ ] **Step 3: 为 MemoryManager 实现 MemoryProvider trait**

在 `gasket/engine/src/agent/memory_manager.rs` 末尾添加:
```rust
#[async_trait]
impl MemoryProvider for MemoryManager {
    async fn load_for_context(&self, query: &MemoryQuery) -> anyhow::Result<MemoryContext> {
        // 委托给现有方法
        self.load_for_context(query).await
    }

    async fn search(&self, query: &str, top_k: usize) -> anyhow::Result<Vec<MemoryHit>> {
        // 需要在 MemoryManager 中暴露 search 方法
        // 如果不存在，创建一个委托给 RetrievalEngine 的方法
        todo!("expose search in MemoryManager or delegate to RetrievalEngine")
    }

    async fn update_from_event(&self, _event: &SessionEvent) -> anyhow::Result<()> {
        // Phase 3: 由 MemoryUpdateHandler 实现
        Ok(())
    }
}
```

注意：`search()` 需要确认 MemoryManager 内部是否有对应的公共方法。如果没有，需要暴露一个。

- [ ] **Step 4: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -10`
Expected: 编译通过（search 的 todo! 会在运行时 panic 但编译无 error）

- [ ] **Step 5: 提交**

```bash
git add gasket/engine/src/agent/memory_provider.rs gasket/engine/src/agent/mod.rs gasket/engine/src/agent/memory_manager.rs
git commit -m "feat(engine): define MemoryProvider trait and implement for MemoryManager"
```

---

### Task 10: 更新 HistoryCoordinator 使用 trait

**Files:**
- Modify: `gasket/engine/src/agent/history_coordinator.rs`

- [ ] **Step 1: 更新 HistoryCoordinator 使用泛型 trait**

```rust
use gasket_storage::{EventStoreTrait, EventFilter};
use crate::agent::memory_provider::MemoryProvider;

pub struct HistoryCoordinator {
    event_store: Arc<dyn EventStoreTrait>,
    compactor: Arc<ContextCompactor>,
    memory: Arc<dyn MemoryProvider>,
}

impl HistoryCoordinator {
    pub fn new(
        event_store: Arc<dyn EventStoreTrait>,
        compactor: Arc<ContextCompactor>,
        memory: Arc<dyn MemoryProvider>,
    ) -> Self {
        Self { event_store, compactor, memory }
    }
}
```

- [ ] **Step 2: 更新路由方法使用 trait 接口**

`save_event` 改用 trait:
```rust
pub async fn save_event(
    &self,
    event: &SessionEvent,
) -> anyhow::Result<()> {
    self.event_store.append(event).await?;
    Ok(())
}
```

`TimeRange` 路由改用 `query_events`:
```rust
HistoryQuery::TimeRange { session_key, start, end } => {
    let events = self.event_store.query_events(EventFilter {
        session_key: Some(session_key),
        time_range: Some((start, end)),
        ..Default::default()
    }).await?;
    Ok(HistoryResult::Events(events))
}
```

- [ ] **Step 3: 更新 PersistentContext 的 coordinator 类型**

在 `context.rs` 中将 coordinator 类型改为:
```rust
pub coordinator: Option<Arc<HistoryCoordinator>>,
```
（HistoryCoordinator 内部已使用 trait，外部类型不变）

- [ ] **Step 4: 运行编译和测试**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`
Expected: 编译通过，所有测试通过

- [ ] **Step 5: 提交**

```bash
git add gasket/engine/src/agent/history_coordinator.rs gasket/engine/src/agent/context.rs
git commit -m "refactor(engine): HistoryCoordinator uses EventStoreTrait and MemoryProvider"
```

---

## Phase 3: 物化引擎接入

> 目标：实现 MaterializationEngine + EventHandler，事件驱动替代直接调用

### Task 11: 定义 EventHandler trait 和 HandlerContext

**Files:**
- Create: `gasket/engine/src/agent/materialization.rs`

- [ ] **Step 1: 创建 materialization.rs — 定义核心类型**

```rust
// gasket/engine/src/agent/materialization.rs

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gasket_storage::EventStoreTrait;
use gasket_types::session_event::SessionEvent;
use serde::{Serialize, Deserialize};

/// Handler 上下文 — 提供事件 + 状态查询能力
pub struct HandlerContext<'a> {
    pub event: &'a SessionEvent,
    pub event_store: &'a dyn EventStoreTrait,
}

/// 事件处理器 trait — 所有 handler 必须实现
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// 基于事件属性判断是否处理（无副作用）
    fn can_handle(&self, event: &SessionEvent) -> bool;

    /// 处理事件
    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()>;

    /// Handler 名称（用于 checkpoint 和日志）
    fn name(&self) -> &str;
}

/// Checkpoint — 记录每个 handler 的处理进度
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub handler_name: String,
    pub last_sequence: i64,
    pub updated_at: DateTime<Utc>,
}
```

- [ ] **Step 2: 实现 CheckpointStore**

在同一个文件中追加:
```rust
use std::sync::Arc;
use gasket_storage::SqliteStore;

/// Checkpoint 存储 — 复用 SqliteStore 的 kv 接口
/// key: "mat:checkpoint:{handler_name}"
/// value: JSON 序列化的 Checkpoint
pub struct CheckpointStore {
    store: Arc<SqliteStore>,
}

impl CheckpointStore {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
    }

    pub async fn load(&self, handler_name: &str) -> Result<Option<Checkpoint>> {
        let key = format!("mat:checkpoint:{}", handler_name);
        let val = self.store.read_raw(&key).await?;
        match val {
            Some(v) => Ok(Some(serde_json::from_str(&v)?)),
            None => Ok(None),
        }
    }

    pub async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let key = format!("mat:checkpoint:{}", checkpoint.handler_name);
        let val = serde_json::to_string(checkpoint)?;
        self.store.write_raw(&key, &val).await?;
        Ok(())
    }
}
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/materialization.rs
git commit -m "feat(engine): define EventHandler trait, HandlerContext, Checkpoint, CheckpointStore"
```

---

### Task 12: 实现失败事件表和 FailedEventStore

**Files:**
- Modify: `gasket/engine/src/agent/materialization.rs`

- [ ] **Step 1: 在 event_store.rs 中添加 failed_events 建表语句**

在 `gasket/storage/src/event_store.rs` 的初始化方法中，添加建表:
```sql
CREATE TABLE IF NOT EXISTS failed_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    error_text TEXT NOT NULL,
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 5,
    next_retry_at TEXT NOT NULL,
    dead_letter INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_failed_events_dedup
    ON failed_events(event_id, handler_name);
```

- [ ] **Step 2: 在 materialization.rs 中定义 FailedEventStore**

```rust
pub struct FailedEventStore {
    pool: SqlitePool,
}

impl FailedEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record_failure(
        &self,
        event_id: &str,
        handler_name: &str,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO failed_events
             (event_id, handler_name, error_text, retry_count, next_retry_at)
             VALUES (?, ?, ?, 0, datetime('now', '+30 seconds'))"
        )
        .bind(event_id)
        .bind(handler_name)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_dead_letter(
        &self,
        event_id: &str,
        handler_name: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE failed_events SET dead_letter = 1
             WHERE event_id = ? AND handler_name = ?"
        )
        .bind(event_id)
        .bind(handler_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add gasket/storage/src/event_store.rs gasket/engine/src/agent/materialization.rs
git commit -m "feat(storage): add failed_events table and FailedEventStore"
```

---

### Task 13: 实现 MaterializationEngine 核心

**Files:**
- Modify: `gasket/engine/src/agent/materialization.rs`
- Modify: `gasket/engine/src/agent/mod.rs`

- [ ] **Step 1: 实现 MaterializationEngine struct 和事件处理循环**

```rust
use tokio::sync::broadcast;

/// 物化引擎 — 事件驱动的处理管道
pub struct MaterializationEngine {
    event_store: Arc<dyn EventStoreTrait>,
    handlers: Vec<Box<dyn EventHandler>>,
    checkpoint_store: CheckpointStore,
    failed_store: FailedEventStore,
}

impl MaterializationEngine {
    pub fn new(
        event_store: Arc<dyn EventStoreTrait>,
        handlers: Vec<Box<dyn EventHandler>>,
        checkpoint_store: CheckpointStore,
        failed_store: FailedEventStore,
    ) -> Self {
        Self { event_store, handlers, checkpoint_store, failed_store }
    }

    /// 启动事件处理循环
    /// 订阅 EventStore broadcast channel，逐事件处理
    pub async fn run(mut self) -> Result<()> {
        let mut rx = self.event_store.subscribe();

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = self.process_event(&event).await {
                        tracing::error!(
                            "MaterializationEngine error processing event {}: {:?}",
                            event.id, e
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("MaterializationEngine lagged {} events, will catch up", n);
                    // Checkpoint 保证不丢失 — 重启后从 checkpoint 恢复
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("MaterializationEngine broadcast closed, shutting down");
                    break;
                }
            }
        }
        Ok(())
    }

    /// 处理单个事件 — 遍历所有匹配的 handler
    async fn process_event(&self, event: &SessionEvent) -> Result<()> {
        let ctx = HandlerContext {
            event,
            event_store: self.event_store.as_ref(),
        };

        for handler in &self.handlers {
            if !handler.can_handle(event) {
                continue;
            }

            match handler.handle(&ctx).await {
                Ok(()) => {
                    // 推进 checkpoint
                    let checkpoint = Checkpoint {
                        handler_name: handler.name().to_string(),
                        last_sequence: event.sequence,
                        updated_at: Utc::now(),
                    };
                    self.checkpoint_store.save(&checkpoint).await?;
                }
                Err(e) => {
                    // 记录失败
                    let error_msg = format!("{:?}", e);
                    self.failed_store
                        .record_failure(&event.id.to_string(), handler.name(), &error_msg)
                        .await?;
                    tracing::error!(
                        "Handler {} failed for event {}: {}",
                        handler.name(), event.id, error_msg
                    );
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: 在 mod.rs 中注册模块并导出**

```rust
pub mod materialization;
pub use materialization::{
    EventHandler, HandlerContext, Checkpoint, CheckpointStore,
    FailedEventStore, MaterializationEngine,
};
```

- [ ] **Step 3: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/materialization.rs gasket/engine/src/agent/mod.rs
git commit -m "feat(engine): implement MaterializationEngine with broadcast event loop"
```

---

### Task 14: 创建 handlers 模块和 IndexingHandler

**Files:**
- Create: `gasket/engine/src/agent/handlers/mod.rs`
- Create: `gasket/engine/src/agent/handlers/indexing_handler.rs`
- Modify: `gasket/engine/src/agent/mod.rs`

- [ ] **Step 1: 创建 handlers/mod.rs**

```rust
// gasket/engine/src/agent/handlers/mod.rs
pub mod indexing_handler;
pub mod compaction_handler;
pub mod memory_update_handler;

pub use indexing_handler::IndexingHandler;
pub use compaction_handler::CompactionHandler;
pub use memory_update_handler::MemoryUpdateHandler;
```

- [ ] **Step 2: 创建 IndexingHandler**

```rust
// gasket/engine/src/agent/handlers/indexing_handler.rs

use anyhow::Result;
use async_trait::async_trait;
use gasket_types::session_event::SessionEvent;
use crate::agent::indexing::IndexingService;
use crate::agent::materialization::{EventHandler, HandlerContext};

/// Indexing Handler — 包装现有 IndexingService
/// 为所有有内容的事件生成 embedding
pub struct IndexingHandler {
    indexing_service: IndexingService,
}

impl IndexingHandler {
    pub fn new(indexing_service: IndexingService) -> Self {
        Self { indexing_service }
    }
}

#[async_trait]
impl EventHandler for IndexingHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        !event.content.is_empty()
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()> {
        let events = std::slice::from_ref(ctx.event);
        self.indexing_service.index_events(events).await;
        Ok(())
    }

    fn name(&self) -> &str {
        "indexing"
    }
}
```

- [ ] **Step 3: 在 agent/mod.rs 中注册 handlers 模块**

```rust
pub mod handlers;
```

- [ ] **Step 4: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -10`
Expected: 编译通过

- [ ] **Step 5: 提交**

```bash
git add gasket/engine/src/agent/handlers/
git commit -m "feat(engine): create handlers module with IndexingHandler"
```

---

### Task 15: 实现 CompactionHandler

**Files:**
- Create: `gasket/engine/src/agent/handlers/compaction_handler.rs`

- [ ] **Step 1: 创建 CompactionHandler**

```rust
// gasket/engine/src/agent/handlers/compaction_handler.rs

use anyhow::Result;
use async_trait::async_trait;
use gasket_types::session_event::{SessionEvent, EventType};
use gasket_storage::EventStoreTrait;
use crate::agent::compactor::ContextCompactor;
use crate::agent::materialization::{EventHandler, HandlerContext};

const COMPACTION_EVENT_THRESHOLD: usize = 50;

/// Compaction Handler — 包装现有 ContextCompactor
/// 在 AssistantMessage 后检查会话事件数，超过阈值触发压缩
pub struct CompactionHandler {
    compactor: std::sync::Arc<ContextCompactor>,
    threshold: usize,
}

impl CompactionHandler {
    pub fn new(compactor: std::sync::Arc<ContextCompactor>) -> Self {
        Self {
            compactor,
            threshold: COMPACTION_EVENT_THRESHOLD,
        }
    }

    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.threshold = threshold;
        self
    }
}

#[async_trait]
impl EventHandler for CompactionHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        matches!(event.event_type, EventType::AssistantMessage)
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()> {
        // 查询当前会话事件数
        let events = ctx.event_store.query_events(
            gasket_storage::EventFilter {
                session_key: Some(ctx.event.session_key.clone()),
                ..Default::default()
            }
        ).await?;

        if events.len() >= self.threshold {
            // 触发压缩 — 委托给 ContextCompactor
            let evicted: Vec<_> = events[..events.len() - 10].to_vec();
            let _ = self.compactor.compact(
                &ctx.event.session_key,
                &evicted,
                &[],
            ).await;
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "compaction"
    }
}
```

- [ ] **Step 2: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 3: 提交**

```bash
git add gasket/engine/src/agent/handlers/compaction_handler.rs
git commit -m "feat(engine): add CompactionHandler wrapping ContextCompactor"
```

---

### Task 16: 实现 MemoryUpdateHandler

**Files:**
- Create: `gasket/engine/src/agent/handlers/memory_update_handler.rs`

- [ ] **Step 1: 创建 MemoryUpdateHandler**

```rust
// gasket/engine/src/agent/handlers/memory_update_handler.rs

use anyhow::Result;
use async_trait::async_trait;
use gasket_types::session_event::{SessionEvent, EventType};
use crate::agent::memory_provider::MemoryProvider;
use crate::agent::materialization::{EventHandler, HandlerContext};

/// Memory Update Handler — 包装现有 MemoryManager
/// 分析 UserMessage 事件，提取知识到记忆文件
pub struct MemoryUpdateHandler {
    memory: std::sync::Arc<dyn MemoryProvider>,
}

impl MemoryUpdateHandler {
    pub fn new(memory: std::sync::Arc<dyn MemoryProvider>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl EventHandler for MemoryUpdateHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        matches!(event.event_type, EventType::UserMessage)
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()> {
        // 委托给 MemoryProvider 的 update_from_event
        self.memory.update_from_event(ctx.event).await
    }

    fn name(&self) -> &str {
        "memory_update"
    }
}
```

- [ ] **Step 2: 运行编译**

Run: `cargo build --package gasket-engine 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 3: 提交**

```bash
git add gasket/engine/src/agent/handlers/memory_update_handler.rs
git commit -m "feat(engine): add MemoryUpdateHandler wrapping MemoryProvider"
```

---

### Task 17: 将 MaterializationEngine 接入系统

**Files:**
- Modify: `gasket/engine/src/agent/context.rs`

- [ ] **Step 1: 在 PersistentContext 中创建和启动 MaterializationEngine**

在 `PersistentContext` 的构建逻辑中:
```rust
// 构建 handlers
let handlers: Vec<Box<dyn EventHandler>> = vec![
    Box::new(IndexingHandler::new(indexing_service.clone())),
    Box::new(CompactionHandler::new(compactor.clone())),
    Box::new(MemoryUpdateHandler::new(memory.clone())),
];

// 构建引擎
let checkpoint_store = CheckpointStore::new(sqlite_store.clone());
let failed_store = FailedEventStore::new(pool.clone());
let engine = MaterializationEngine::new(
    event_store.clone(),
    handlers,
    checkpoint_store,
    failed_store,
);

// 在后台 tokio task 中运行
tokio::spawn(async move {
    if let Err(e) = engine.run().await {
        tracing::error!("MaterializationEngine error: {:?}", e);
    }
});
```

注意：具体集成位置取决于 PersistentContext 的构建方式。需要在 `AgentContext::persistent()` 或对应的 builder 中添加。

- [ ] **Step 2: 运行编译和测试**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`
Expected: 编译通过，所有测试通过

- [ ] **Step 3: 提交**

```bash
git add gasket/engine/src/agent/context.rs
git commit -m "feat(engine): wire MaterializationEngine into PersistentContext"
```

---

## Phase 4: 清理

> 目标：移除 Agent Loop 中的旧直接方法调用，Coordinator 是唯一接口

### Task 18: 更新 AgentLoop 使用 HistoryCoordinator

**Files:**
- Modify: `gasket/engine/src/agent/loop_.rs`

- [ ] **Step 1: 在 AgentLoop struct 中添加 coordinator 字段**

```rust
pub struct AgentLoop {
    // ... 现有字段 ...
    coordinator: Option<Arc<HistoryCoordinator>>,
}
```

- [ ] **Step 2: 逐步替换直接调用**

在 `prepare_pipeline()` 或 `process_direct()` 中，将:
```rust
let history = context.get_history();
```
替换为:
```rust
let result = coordinator.query(
    HistoryQuery::SessionContext { session_key, token_budget }
).await?;
```

对每个现有调用点逐一替换，每替换一个就编译测试一次。

主要替换点（用 grep 确认）:
- `context.get_history()` → `HistoryQuery::SessionContext`
- `context.load_latest_summary()` → `HistoryQuery::LatestSummary`
- `context.recall_history()` → `HistoryQuery::SemanticSearch`
- `memory_manager.load_for_context()` → `HistoryQuery::MemoryContext`
- `indexing_service.index_events()` → 由 MaterializationEngine 自动处理

- [ ] **Step 3: 运行编译和测试**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`
Expected: 编译通过，所有测试通过

- [ ] **Step 4: 提交**

```bash
git add gasket/engine/src/agent/loop_.rs
git commit -m "refactor(engine): AgentLoop uses HistoryCoordinator for all history queries"
```

---

### Task 19: 标记旧方法为 deprecated

**Files:**
- Modify: `gasket/engine/src/agent/context.rs`

- [ ] **Step 1: 为旧直接方法添加 #[deprecated] 标记**

```rust
impl PersistentContext {
    #[deprecated(since = "0.2.0", note = "Use HistoryCoordinator::query(SessionContext) instead")]
    pub async fn get_history(&self, key: &str, branch: Option<&str>) -> Vec<SessionEvent> {
        // 保留现有实现，委托给内部逻辑
    }

    #[deprecated(since = "0.2.0", note = "Use HistoryCoordinator::query(LatestSummary) instead")]
    pub async fn load_latest_summary(&self, session_key: &str, branch: &str) -> Option<String> {
        // 保留现有实现
    }

    #[deprecated(since = "0.2.0", note = "Use HistoryCoordinator::query(SemanticSearch) instead")]
    pub async fn recall_history(&self, key: &str, query_embedding: &[f32], top_k: usize) -> anyhow::Result<Vec<String>> {
        // 保留现有实现
    }
}
```

- [ ] **Step 2: 运行编译（预期有 deprecation warnings）**

Run: `cargo build --workspace 2>&1 | grep "deprecated" | head -10`
Expected: 看到新添加的 deprecation warnings，但无 error

- [ ] **Step 3: 提交**

```bash
git add gasket/engine/src/agent/context.rs
git commit -m "refactor(engine): deprecate old direct methods in favor of HistoryCoordinator"
```

---

### Task 20: 最终集成验证

**Files:**
- 全项目

- [ ] **Step 1: 运行完整测试套件**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: 所有测试通过

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy --workspace 2>&1 | grep -v "cosine_similarity" | grep -v "bootstrap_tokens" | grep -v "scenario_tokens" | tail -10`
Expected: 无新的 clippy warnings

- [ ] **Step 3: 检查 git diff 确认变更范围**

Run: `git diff HEAD~15 --stat`
Expected: 新文件约 6 个，修改文件约 6 个

- [ ] **Step 4: 最终提交**

```bash
git add -A
git commit -m "chore: history module boundary refactor complete — Phase 1-4

Architecture changes:
- EventStoreTrait: narrow interface (append, query, subscribe)
- MemoryProvider: extracted from MemoryManager
- HistoryCoordinator: single entry point for AgentLoop
- MaterializationEngine: event-driven pipeline with checkpoint + retry
- 4-phase migration: facade → traits → engine → cleanup

Fixes: EventStore scope creep, memory/compaction overlap,
       agent loop coupling, data flow clarity"
```
