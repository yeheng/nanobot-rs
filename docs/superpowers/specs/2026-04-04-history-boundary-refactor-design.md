# 历史记录模块边界重构设计

## 概述

本设计是对历史记录模块的**接口优先边界重构**，通过提取 trait、引入协调器、添加物化引擎来理清模块职责边界，而非全面重写。

本设计是 [2026-04-04-history-redesign-design.md](./2026-04-04-history-redesign-design.md)（完整 CQRS 方案）的**轻量替代**，采纳其核心洞察（单一查询入口、事件驱动物化），但通过包装现有组件而非重建来实现。

## 问题陈述

当前系统存在四个边界问题：

1. **EventStore 职责膨胀** — 同时处理追加、查询、截断、摘要管理、embedding 存储、会话追踪
2. **Memory/Compaction 重叠** — MemoryManager、ContextCompactor、IndexingService 在 embedding 和 token budget 上有重叠关注点
3. **Agent Loop 直接耦合** — 直接调用 EventStore、Compactor、MemoryManager、IndexingService 四个独立子系统
4. **数据流不清晰** — prepare_pipeline → process_history → compaction → memory injection 链路难以追踪

## 设计目标

- **单一入口**: HistoryCoordinator 是 Agent Loop 的唯一历史相关接口
- **职责收缩**: EventStore 只做追加 + 查询，其他职责移出
- **事件驱动**: 新事件通过 MaterializationEngine 驱动索引、压缩、记忆更新
- **增量迁移**: 四阶段迁移，每阶段独立可测可部署

## 核心架构

### 重构后数据流

```
Agent Loop
    ↓ [唯一入口]
HistoryCoordinator
    ↙           ↓           ↘
EventStore   Compactor   MemoryProvider
    ↓ [事件发布]
MaterializationEngine
    ↙       ↓        ↘       ↘
Indexing  Compaction  Memory  Dedup
Handler   Handler    Handler  Handler
```

### 与原 CQRS 方案的对应关系

| 原 CQRS 概念 | 本设计对应 | 实现方式 |
|---|---|---|
| EventStore | EventStore trait | 从现有 SqliteEventStore 提取接口 |
| ViewCoordinator | HistoryCoordinator | 薄路由层，包装现有组件 |
| SessionView | ContextCompactor | 保留现有 LSM-tree 实现 |
| KnowledgeView | MemoryProvider trait | 从 MemoryManager 提取接口 |
| DecisionView | 暂不实现 | YAGNI — 需要时添加新 EventHandler |
| MaterializationEngine | MaterializationEngine | 新增，包装现有组件为 EventHandler |

## 类型定义

以下类型在组件设计中引用，统一定义在此：

```rust
/// 追加事件的输入类型（不含 id、created_at、sequence，由 EventStore 生成）
pub struct NewEvent {
    pub session_key: String,
    pub event_type: EventType,
    pub content: String,
    pub branch: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub metadata: EventMetadata,
}

/// 事件 ID，复用现有 Uuid 类型
pub type EventId = Uuid;

/// 语义搜索结果（MemoryFile 的轻量视图）
pub struct MemoryHit {
    pub id: String,
    pub title: String,
    pub content: String,
    pub scenario: Scenario,
    pub frequency: Frequency,
    pub tags: Vec<String>,
    pub score: f64,  // 综合得分 = embedding_score × frequency_bonus
    pub token_count: usize,
}
```

## 组件设计

### 1. EventStore Trait（职责收缩）

从现有 `SqliteEventStore` 提取窄接口：

```rust
#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, event: NewEvent) -> Result<EventId>;
    async fn query(&self, filter: EventFilter) -> Result<Vec<SessionEvent>>;
    fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;
}

pub struct EventFilter {
    pub session_key: Option<String>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub event_types: Option<Vec<EventType>>,
    pub event_ids: Option<Vec<Uuid>>,      // 替代 get_events_by_ids()
    pub limit: Option<usize>,
    pub branch: Option<String>,
}
```

**新增字段**:

```rust
pub struct SessionEvent {
    // ... 现有字段 ...
    pub sequence: i64,  // 单调递增序列号，用于增量同步和 checkpoint
}
```

**sequence 列迁移计划**:

```sql
-- Phase 2 中执行，向后兼容
ALTER TABLE session_events ADD COLUMN sequence INTEGER;

-- 回填：按 created_at 排序生成序列号
UPDATE session_events SET sequence = subquery.row_num
FROM (
    SELECT id, ROW_NUMBER() OVER (ORDER BY created_at ASC) as row_num
    FROM session_events
) AS subquery
WHERE session_events.id = subquery.id;

-- 后续插入使用 SQLite AUTOINCREMENT 或应用层生成
```

**Broadcast channel 配置**:

```rust
// SqliteEventStore 内部持有 sender
pub struct SqliteEventStore {
    pool: SqlitePool,
    tx: broadcast::Sender<SessionEvent>,  // buffer = 64
}

impl SqliteEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        let (tx, _) = broadcast::channel(64);  // 64 事件缓冲
        Self { pool, tx }
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.tx.subscribe()
    }
}
```

- **所有权**: `SqliteEventStore` 拥有 `Sender`，生命周期与 store 相同
- **多订阅者**: broadcast 支持多个 Receiver。MaterializationEngine 和未来组件可各自订阅
- **慢消费者**: 超过 buffer 的事件自动丢弃，Checkpoint 机制兜底恢复

**移出的职责**:

| 原位置 | 移至 | 原因 |
|---|---|---|
| `get_latest_summary()` | `ContextCompactor` | 摘要生成和管理是压缩的职责 |
| Token 计数 | `AgentContext::save_event()` | 调用者应负责 token 计算 |
| Embedding 生成 | `IndexingHandler` | 物化引擎统一处理 |
| 会话创建/追踪 | `AgentContext` | 会话生命周期由上下文管理 |

**向后兼容**: `SqliteEventStore` 保留所有现有方法。添加 `impl EventStore for SqliteEventStore` 作为薄委托层。旧代码继续工作，新代码使用 trait。

### 2. MemoryProvider Trait（接口提取）

从现有 `MemoryManager` 提取查询接口。**接口签名与现有 MemoryManager 匹配，使用 async + MemoryQuery**：

```rust
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// 三阶段加载（bootstrap/scenario/on-demand），复用现有 MemoryQuery
    async fn load_for_context(
        &self,
        query: &MemoryQuery,
    ) -> Result<MemoryContext>;

    /// 语义搜索，复用现有 RetrievalEngine
    async fn search(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<MemoryHit>>;

    /// 从事件中提取知识（由 MemoryUpdateHandler 调用）
    async fn update(&self, event: &SessionEvent) -> Result<()>;
}
```

现有 `MemoryManager` 实现 `MemoryProvider`。三阶段加载策略（bootstrap/scenario/on-demand）保持不变。`MemoryQuery` 和 `MemoryContext` 复用现有定义。

### 3. HistoryCoordinator（单一入口）

薄路由层，是 Agent Loop 的唯一历史相关接口：

```rust
pub struct HistoryCoordinator {
    event_store: Arc<dyn EventStore>,
    compactor: Arc<ContextCompactor>,
    memory: Arc<dyn MemoryProvider>,
    engine: Arc<MaterializationEngine>,
}

pub enum HistoryQuery {
    /// "给我这个会话的最近上下文，在 token 预算内"
    /// 路由到 ContextCompactor（LSM-tree: L0 events + L1 summary）
    SessionContext {
        session_key: String,
        token_budget: usize,
    },
    /// "获取最新摘要" — 替代原 context.load_latest_summary()
    /// 路由到 ContextCompactor::load_summary()
    LatestSummary {
        session_key: String,
    },
    /// "跨会话语义搜索" — 替代原 context.recall_history()
    /// 路由到 MemoryProvider::search()
    SemanticSearch {
        query: String,
        top_k: usize,
    },
    /// "三阶段记忆加载" — 替代原 memory_manager.load_for_context()
    /// 路由到 MemoryProvider::load_for_context()
    MemoryContext {
        query: MemoryQuery,
    },
    /// "查看指定时间范围的原始事件"
    /// 路由到 EventStore::query()
    TimeRange {
        session_key: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

pub enum HistoryResult {
    Context(Vec<ChatMessage>),      // Compactor 返回
    Summary(Option<String>),        // Compactor 返回
    Memories(Vec<MemoryHit>),       // MemoryProvider 返回
    MemoryContext(MemoryContext),    // MemoryProvider 返回
    Events(Vec<SessionEvent>),      // EventStore 返回
}
```

**路由逻辑**:

| 查询类型 | 路由目标 | 替代的现有调用 |
|---|---|---|
| `SessionContext` | `ContextCompactor` | `context.get_history()` + `process_history()` |
| `LatestSummary` | `ContextCompactor::load_summary()` | `context.load_latest_summary()` |
| `SemanticSearch` | `MemoryProvider::search()` | `context.recall_history()` |
| `MemoryContext` | `MemoryProvider::load_for_context()` | `memory_manager.load_for_context()` |
| `TimeRange` | `EventStore::query()` | `context.get_history()` (原始模式) |

```rust
impl HistoryCoordinator {
    pub async fn query(&self, query: HistoryQuery) -> Result<HistoryResult> {
        match query {
            HistoryQuery::SessionContext { session_key, token_budget } => {
                let context = self.compactor.get_context(&session_key, token_budget).await?;
                Ok(HistoryResult::Context(context))
            }
            HistoryQuery::LatestSummary { session_key } => {
                let summary = self.compactor.load_summary(&session_key).await?;
                Ok(HistoryResult::Summary(summary))
            }
            HistoryQuery::SemanticSearch { query, top_k } => {
                let hits = self.memory.search(&query, top_k).await?;
                Ok(HistoryResult::Memories(hits))
            }
            HistoryQuery::MemoryContext { query } => {
                let ctx = self.memory.load_for_context(&query).await?;
                Ok(HistoryResult::MemoryContext(ctx))
            }
            HistoryQuery::TimeRange { session_key, start, end } => {
                let events = self.event_store.query(EventFilter {
                    session_key: Some(session_key),
                    time_range: Some((start, end)),
                    ..Default::default()
                }).await?;
                Ok(HistoryResult::Events(events))
            }
        }
    }

    pub async fn save_event(&self, event: NewEvent) -> Result<EventId> {
        let id = self.event_store.append(event).await?;
        // MaterializationEngine 通过 broadcast 接收通知
        Ok(id)
    }
}
```

**关键约束**: HistoryCoordinator 是路由器，不是处理器。允许简单的类型转换（如 `SessionEvent` → `ChatMessage`），但不包含业务逻辑。所有计算委托给现有组件。

### 4. MaterializationEngine（物化引擎）

事件驱动的处理管道，将现有组件包装为 EventHandler：

```rust
pub struct MaterializationEngine {
    event_store: Arc<dyn EventStore>,  // handler 可查询状态
    handlers: Vec<Box<dyn EventHandler>>,
    checkpoint_store: CheckpointStore,
    failed_store: FailedEventStore,
}

/// Handler 上下文 — 提供事件 + 状态查询能力
pub struct HandlerContext<'a> {
    pub event: &'a SessionEvent,
    pub event_store: &'a dyn EventStore,  // 用于查询会话状态
}

#[async_trait]
pub trait EventHandler: Send + Sync {
    /// 基于事件属性判断是否处理（无副作用）
    fn can_handle(&self, event: &SessionEvent) -> bool;
    /// 处理事件，可通过 ctx.event_store 查询额外状态
    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()>;
    fn name(&self) -> &str;
}

pub struct Checkpoint {
    pub handler_name: String,
    pub last_sequence: i64,
    pub updated_at: DateTime<Utc>,
}
```

**内置 Handlers（包装现有组件）**:

| Handler | 包装 | `can_handle()` 条件 | 行为 |
|---|---|---|---|
| `IndexingHandler` | `IndexingService` | `event.content.len() > 0` | 生成 embedding |
| `CompactionHandler` | `ContextCompactor` | `event.event_type == AssistantMessage`（每次响应后检查压缩） | 通过 `ctx.event_store` 查询会话事件数，超过阈值触发压缩 |
| `MemoryUpdateHandler` | `MemoryManager` | `event.event_type == UserMessage`（分析用户消息提取知识） | 解析 content 识别决策、模式、偏好等 |
| `DedupHandler` | `DedupScanner` | 由 `MemoryUpdateHandler` 完成后间接触发 | 通过 handler 间通知机制触发 |

**Handler 执行顺序与依赖**:

```
IndexingHandler → CompactionHandler → MemoryUpdateHandler → DedupHandler
     ↓                  ↓                    ↓
  (无依赖)       (依赖 embedding)      (无前置依赖)
```

- `IndexingHandler` 先执行，确保 embedding 可用
- `CompactionHandler` 可查询 event_store 获取会话状态
- `MemoryUpdateHandler` 独立执行，不依赖前两个 handler
- `DedupHandler` 由 `MemoryUpdateHandler` 完成后间接触发（非直接依赖）

**处理流程**:

1. 订阅 EventStore 的 broadcast channel
2. 对每个事件，按顺序调用 handlers — `can_handle()` 过滤，`handle()` 处理
3. 所有 handler 成功后推进 checkpoint
4. 失败事件记录到 `failed_events` 表，指数退避重试

**Checkpoint 存储**（复用 SqliteStore 的现有 kv 接口）:

```rust
/// 复用 SqliteStore::kv_get() / kv_set() 存储 checkpoint
/// key 格式: "mat:checkpoint:{handler_name}"
/// value: serde_json serialize 的 Checkpoint 结构
pub struct CheckpointStore {
    store: Arc<SqliteStore>,
}

impl CheckpointStore {
    pub async fn load(&self, handler_name: &str) -> Result<Option<Checkpoint>> {
        let key = format!("mat:checkpoint:{}", handler_name);
        let val = self.store.kv_get(&key).await?;
        // serde_json deserialize
    }

    pub async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let key = format!("mat:checkpoint:{}", checkpoint.handler_name);
        let val = serde_json::to_string(checkpoint)?;
        self.store.kv_set(&key, &val).await?;
        Ok(())
    }
}
```

**失败事件表**:

```sql
CREATE TABLE IF NOT EXISTS failed_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    error_text TEXT NOT NULL,
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 5,
    next_retry_at TIMESTAMP NOT NULL,
    dead_letter BOOLEAN DEFAULT FALSE,  -- 超过重试次数后标记
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 防止同一事件 + handler 重复记录
CREATE UNIQUE INDEX IF NOT EXISTS idx_failed_events_dedup
    ON failed_events(event_id, handler_name);
```

## Agent Loop 集成

### 变更前后对比

**Before**（agent loop 直接调用 5+ 个接口）:
```rust
let history = context.get_history();
let processed = process_history(history, budget);
let summary = context.load_latest_summary();
let memories = memory_manager.load_for_context(&memory_query);
let recalled = context.recall_history(&query);
indexing_service.index_events(&events);
```

**After**（agent loop 只与 coordinator 交互）:
```rust
let context = coordinator.query(
    HistoryQuery::SessionContext { session_key, token_budget: 8000 }
).await?;
let summary = coordinator.query(
    HistoryQuery::LatestSummary { session_key }
).await?;
let memories = coordinator.query(
    HistoryQuery::MemoryContext { query: memory_query }
).await?;
let recalled = coordinator.query(
    HistoryQuery::SemanticSearch { query, top_k: 10 }
).await?;
// Indexing 由 MaterializationEngine 自动处理
```

### AgentLoop 结构变更

```rust
pub struct AgentLoop {
    // ... 现有字段 ...
    coordinator: Arc<HistoryCoordinator>,  // 替代 4 个独立引用
}
```

### AgentContext 集成

现有 `AgentContext` enum dispatch（Persistent/Stateless）模式保持不变。变更仅影响 `PersistentContext` 内部实现：

```rust
pub enum AgentContext {
    Persistent(PersistentContext),
    Stateless(StatelessContext),
}

// PersistentContext 内部使用 coordinator，但 AgentContext 的公共 API 不变
pub struct PersistentContext {
    coordinator: Arc<HistoryCoordinator>,  // 替代直接的 event_store + compactor
    // ...
}
```

## 迁移策略

### 四阶段增量迁移

**Phase 1: Facade 引入（1 周）**
- 创建 `HistoryCoordinator` 作为现有直接调用的门面
- Agent loop 通过 coordinator 调用，行为不变
- 验证：所有现有测试通过
- **回滚策略**: 删除 coordinator 调用，恢复直接调用（无数据变更）

**Phase 2: Trait 提取（1 周）**
- 从 `SqliteEventStore` 提取 `EventStore` trait
- 从 `MemoryManager` 提取 `MemoryProvider` trait
- Coordinator 依赖 trait 而非具体类型
- 添加 `sequence` 列 + 回填（见 EventStore 部分）
- 验证：trait 实现正确委托
- **回滚策略**: trait impl 只是委托层，删除 trait 并恢复具体类型引用即可。`sequence` 列为新增列，不影响现有查询。
- **停机**: 无需停机。ALTER TABLE + UPDATE 在 SQLite 中即时完成。

**Phase 3: 物化引擎接入（1 周）**
- 实现 `MaterializationEngine` + EventHandler trait
- 创建 IndexingHandler、CompactionHandler 包装现有组件
- EventStore 添加 broadcast channel
- 从 agent loop 移除对 IndexingService、MemoryWatcher 的直接调用
- 验证：事件正确传播，checkpoint 正确推进
- **回滚策略**: 关闭 MaterializationEngine 的 broadcast 订阅，恢复 agent loop 中的直接调用。failed_events 表为新增，不影响现有数据。
- **停机**: 无需停机。broadcast channel 为内存结构，重启即生效。

**Phase 4: 清理（0.5 周）**
- 移除 agent loop 中的旧直接方法调用
- Coordinator 是唯一接口
- 旧方法标记 `#[deprecated]`（不立即移除，留一个版本周期）
- 验证：完整集成测试通过

### 向后兼容

- 所有现有公共 API 保留，内部委托给新接口
- `MemoryManager` 保留现有方法签名，额外实现 `MemoryProvider` trait
- `SqliteEventStore` 保留现有方法，额外实现 `EventStore` trait
- 旧代码路径标记 `#[deprecated]` 而非立即移除

## 错误处理

### 一致性保证

- EventStore 写入成功 = 事务提交
- 物化引擎更新失败不影响写入（最终一致性）
- Checkpoint 机制保证 handler 最终追上
- 所有 EventHandler 必须幂等（使用 `event.sequence` 去重，handler 自行维护已处理集合或使用 upsert 语义）

### 故障恢复

- MaterializationEngine 启动时从 checkpoint 恢复
- 失败事件记录到 `failed_events` 表
- 定期重试（指数退避，最大重试 5 次）
- 超过重试限制的事件标记为 `dead_letter = TRUE`，供人工检查

### 边界场景

**Checkpoint 偏斜**（Handler A 在 sequence 100，Handler B 在 95，系统崩溃）:
- 每个 handler 独立维护 checkpoint
- 重启后各 handler 从自己的 checkpoint 恢复
- Handler A 从 101 开始，Handler B 从 96 开始
- 幂等性保证重复处理安全

**毒丸事件**（导致 handler 反复失败）:
- 重试 5 次后标记 `dead_letter = TRUE`
- dead_letter 事件不再重试，但记录在案
- 监控指标跟踪 dead_letter 数量，触发告警

**部分 handler 完成**（3 个 handler 中 2 个成功，1 个失败）:
- 成功的 handler 各自推进 checkpoint
- 失败的 handler 记录到 failed_events
- 下次重试仅处理失败的 handler（checkpoint 不同步不影响）

### 监控指标

```rust
pub struct EngineMetrics {
    pub event_lag: i64,                          // 最慢 handler 落后的事件数
    pub processing_latency_ms: f64,              // 处理延迟
    pub failed_events_count: usize,              // 失败事件数
    pub dead_letter_count: usize,                // 毒丸事件数
    pub handler_status: HashMap<String, HandlerStatus>,
}

pub struct HandlerStatus {
    pub last_sequence: i64,                      // handler 的 checkpoint
    pub lag: i64,                                // 落后当前序列多少
    pub last_error: Option<String>,              // 最近一次错误
    pub last_processed_at: DateTime<Utc>,        // 最近处理时间
}
```

## 与原 CQRS 方案的关系

本设计是原方案的**增量实现路径**。如果未来需要完整 CQRS：

1. `HistoryCoordinator` → `ViewCoordinator`（扩展路由逻辑）
2. `MemoryProvider` → `KnowledgeView`（添加独立存储）
3. 新增 `DecisionView`（添加新的 EventHandler）
4. `CheckpointStore` → 独立 SQLite 表（从 kv store 升级）

每个升级步骤都是独立的，不需要推翻现有实现。

## 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Handler 处理延迟影响下次查询 | 中 | SessionContext 走 Compactor（同步），不受异步 handler 影响 |
| Broadcast 慢消费者丢失事件 | 低 | 设置合理 buffer（64），Checkpoint 兜底恢复 |
| 迁移过程中新旧路径并存 | 中 | Phase 1 门面模式确保行为不变，逐步切换 |
| Handler 幂等性实现遗漏 | 中 | 代码审查 + 集成测试验证重复处理安全性 |

## 总结

本设计通过四个核心变更解决边界问题：

- **EventStore trait** — 职责从 6 项收缩到 3 项（append、query、subscribe）
- **HistoryCoordinator** — Agent Loop 从 4 个调用点缩减到 1 个
- **MaterializationEngine** — 事件驱动处理替代直接调用
- **四阶段迁移** — 每阶段独立可测，无 "big bang" 风险

估计工期：3-4 周（vs 原 CQRS 方案的 8-12 周）
