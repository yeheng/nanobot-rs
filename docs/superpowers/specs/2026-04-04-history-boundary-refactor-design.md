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

## 组件设计

### 1. EventStore Trait（职责收缩）

从现有 `SqliteEventStore` 提取窄接口：

```rust
pub trait EventStore: Send + Sync {
    fn append(&self, event: NewEvent) -> Result<EventId>;
    fn query(&self, filter: EventFilter) -> Result<Vec<SessionEvent>>;
    fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;
}

pub struct EventFilter {
    pub session_key: Option<String>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub event_types: Option<Vec<EventType>>,
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

**移出的职责**:

| 原位置 | 移至 | 原因 |
|---|---|---|
| `get_latest_summary()` | `ContextCompactor` | 摘要生成和管理是压缩的职责 |
| Token 计数 | `AgentContext::save_event()` | 调用者应负责 token 计算 |
| Embedding 生成 | `IndexingHandler` | 物化引擎统一处理 |
| 会话创建/追踪 | `AgentContext` | 会话生命周期由上下文管理 |

**向后兼容**: `SqliteEventStore` 保留所有现有方法。添加 `impl EventStore for SqliteEventStore` 作为薄委托层。旧代码继续工作，新代码使用 trait。

### 2. MemoryProvider Trait（接口提取）

从现有 `MemoryManager` 提取查询接口：

```rust
pub trait MemoryProvider: Send + Sync {
    fn load_for_context(
        &self,
        scenario: &Scenario,
        budget: usize,
    ) -> Result<MemoryContext>;

    fn search(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<MemoryHit>>;

    fn update(&self, event: &SessionEvent) -> Result<()>;
}
```

现有 `MemoryManager` 实现 `MemoryProvider`。三阶段加载策略（bootstrap/scenario/on-demand）保持不变。

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
    SessionContext {
        session_key: String,
        token_budget: usize,
    },
    /// "跨会话搜索相关知识"
    SemanticSearch {
        query: String,
        top_k: usize,
    },
    /// "查看指定时间范围的原始事件"
    TimeRange {
        session_key: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

pub enum HistoryResult {
    Context(Vec<ChatMessage>),
    Memories(Vec<MemoryHit>),
    Events(Vec<SessionEvent>),
}

impl HistoryCoordinator {
    pub fn query(&self, query: HistoryQuery) -> Result<HistoryResult> {
        match query {
            HistoryQuery::SessionContext { session_key, token_budget } => {
                let context = self.compactor.get_context(&session_key, token_budget)?;
                Ok(HistoryResult::Context(context))
            }
            HistoryQuery::SemanticSearch { query, top_k } => {
                let hits = self.memory.search(&query, top_k)?;
                Ok(HistoryResult::Memories(hits))
            }
            HistoryQuery::TimeRange { session_key, start, end } => {
                let events = self.event_store.query(EventFilter {
                    session_key: Some(session_key),
                    time_range: Some((start, end)),
                    ..Default::default()
                })?;
                Ok(HistoryResult::Events(events))
            }
        }
    }

    pub fn save_event(&self, event: NewEvent) -> Result<EventId> {
        let id = self.event_store.append(event)?;
        // MaterializationEngine 通过 broadcast 接收通知
        Ok(id)
    }
}
```

**关键约束**: HistoryCoordinator 是路由器，不是处理器。不包含业务逻辑。所有计算委托给现有组件。

### 4. MaterializationEngine（物化引擎）

事件驱动的处理管道，将现有组件包装为 EventHandler：

```rust
pub struct MaterializationEngine {
    handlers: Vec<Box<dyn EventHandler>>,
    checkpoint_store: Arc<CheckpointStore>,
    failed_store: Arc<FailedEventStore>,
}

pub trait EventHandler: Send + Sync {
    fn can_handle(&self, event: &SessionEvent) -> bool;
    fn handle(&self, event: &SessionEvent) -> Result<()>;
    fn name(&self) -> &str;
}

pub struct Checkpoint {
    pub handler_name: String,
    pub last_sequence: i64,
    pub updated_at: DateTime<Utc>,
}
```

**内置 Handlers（包装现有组件）**:

| Handler | 包装 | 触发条件 | 行为 |
|---|---|---|---|
| `IndexingHandler` | `IndexingService` | 所有有内容的事件 | 生成 embedding |
| `CompactionHandler` | `ContextCompactor` | 会话事件数超过阈值 | 触发 LSM-tree 压缩 |
| `MemoryUpdateHandler` | `MemoryManager` | 决策、模式发现等显著事件 | 提取知识到记忆文件 |
| `DedupHandler` | `DedupScanner` | 新记忆文件创建 | 触发相似度检查 |

**处理流程**:

1. 订阅 EventStore 的 broadcast channel
2. 对每个事件，迭代 handlers — `can_handle()` 过滤，`handle()` 处理
3. 所有 handler 成功后推进 checkpoint
4. 失败事件记录到 `failed_events` 表，指数退避重试

**Checkpoint 存储**:

```rust
// 使用现有 kv store，无需新表
impl CheckpointStore {
    fn key_for(handler_name: &str) -> String {
        format!("checkpoint:{}", handler_name)
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
    next_retry_at TIMESTAMP NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

## Agent Loop 集成

### 变更前后对比

**Before**:
```rust
// Agent loop 直接调用 4 个子系统
let history = context.get_history();
let processed = process_history(history, budget);
let summary = compactor.load_summary();
let memories = memory_manager.load_for_context(scenario, budget);
indexing_service.index_events(&events);
```

**After**:
```rust
// Agent loop 只与 coordinator 交互
let context = coordinator.query(
    HistoryQuery::SessionContext { session_key, token_budget: 8000 }
)?;
let memories = coordinator.query(
    HistoryQuery::SemanticSearch { query, top_k: 10 }
)?;
```

### AgentLoop 结构变更

```rust
pub struct AgentLoop {
    // ... 现有字段 ...
    coordinator: Arc<HistoryCoordinator>,  // 替代 4 个独立引用
}
```

## 迁移策略

### 四阶段增量迁移

**Phase 1: Facade 引入（1 周）**
- 创建 `HistoryCoordinator` 作为现有直接调用的门面
- Agent loop 通过 coordinator 调用，行为不变
- 验证：所有现有测试通过

**Phase 2: Trait 提取（1 周）**
- 从 `SqliteEventStore` 提取 `EventStore` trait
- 从 `MemoryManager` 提取 `MemoryProvider` trait
- Coordinator 依赖 trait 而非具体类型
- 验证：trait 实现正确委托

**Phase 3: 物化引擎接入（1 周）**
- 实现 `MaterializationEngine` + EventHandler trait
- 创建 IndexingHandler、CompactionHandler 包装现有组件
- EventStore 添加 broadcast channel
- 从 agent loop 移除对 IndexingService、MemoryWatcher 的直接调用
- 验证：事件正确传播，checkpoint 正确推进

**Phase 4: 清理（0.5 周）**
- 移除 agent loop 中的旧直接方法调用
- Coordinator 是唯一接口
- 移除不再需要的旧方法（标记 deprecated 先）
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
- 所有 EventHandler 必须幂等（使用 `event.sequence` 去重）

### 故障恢复

- MaterializationEngine 启动时从 checkpoint 恢复
- 失败事件记录到 `failed_events` 表
- 定期重试（指数退避，最大重试 5 次）
- 超过重试限制的事件标记为 `dead_letter`，供人工检查

### 监控指标

```rust
pub struct EngineMetrics {
    pub event_lag: i64,              // handler 落后的事件数
    pub processing_latency_ms: f64,  // 处理延迟
    pub failed_events_count: usize,  // 失败事件数
    pub handler_status: HashMap<String, HandlerStatus>,
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
