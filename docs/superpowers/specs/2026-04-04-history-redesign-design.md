# 历史记录模块重新设计规格

## 概述

本规格描述 Gasket 历史记录模块的架构重新设计，采用**事件溯源 + 物化视图（CQRS）**模式，解决当前 SessionEvent 和 Memory 系统之间的边界模糊问题。

## 问题陈述

当前系统存在以下架构问题：

1. **数据转换不明确** - SessionEvent 何时提升为 Memory？由谁决定？如何转换？
2. **检索路径重叠** - Agent 需要历史时，应查 SessionEvent 还是 Memory？两者如何协同？
3. **生命周期管理混乱** - SessionEvent 何时过期？过期后是删除还是归档？
4. **职责边界模糊** - SessionEvent 既做原始日志又做语义搜索，功能重叠

## 设计目标

- **单一真相源**: EventStore 是唯一写入目标，所有其他存储都是派生视图
- **职责清晰**: EventStore = 事实记录，Views = 知识提取
- **可重建性**: 任何视图损坏都可从 EventStore 重放恢复
- **查询优化**: 不同查询类型路由到专门优化的视图

## 核心架构

### 数据流

```
用户消息 → EventStore.append()
              ↓ [事件发布]
    MaterializationEngine
         ↙    ↓    ↘
SessionView  KnowledgeView  DecisionView
```

### 关键原则

1. **写入路径单一**: 所有数据只写入 EventStore
2. **读取路径多样**: 根据查询类型路由到最优视图
3. **最终一致性**: 视图更新异步，延迟通常 < 100ms
4. **幂等处理**: 视图更新逻辑必须幂等，支持重试和重放

## 组件设计

### 1. EventStore（事件存储）

**职责收缩**: 从"历史管理器"收缩为"事件日志"

**保留功能**:
- `append_event()` - 追加事件（唯一写入接口）
- `get_events(session_key, time_range)` - 按时间范围查询原始事件
- `subscribe_events(callback)` - 事件订阅（新增）
- `replay_events(from_time, to_time)` - 事件重放（用于视图重建）

**移除功能**:
- ❌ 历史截断逻辑（移到 SessionView）
- ❌ 语义搜索（移到 KnowledgeView）
- ❌ 摘要生成（移到 MaterializationEngine）
- ❌ Token 预算管理（移到上层）

**数据模型变更**:

```rust
pub struct SessionEvent {
    pub id: Uuid,
    pub session_key: String,
    pub event_type: EventType,
    pub content: String,
    pub created_at: DateTime<Utc>,
    
    // 新增：事件序列号（单调递增，用于增量同步）
    pub sequence: i64,
    
    // 新增：事件版本（用于 schema 演化）
    pub schema_version: u32,
    
    // 保留但不主动使用
    pub embedding: Option<Vec<f32>>,
    pub metadata: EventMetadata,
}
```

**事件订阅机制**:

```rust
// 订阅接口
EventStore::subscribe(|event: SessionEvent| {
    materialization_engine.process(event);
});
```

### 2. MaterializationEngine（物化引擎）

**职责**: 监听 EventStore 事件流，增量更新各个物化视图

**架构**:

```rust
pub struct MaterializationEngine {
    event_store: Arc<EventStore>,
    handlers: Vec<Box<dyn EventHandler>>,
    checkpoint_store: CheckpointStore,
}

pub trait EventHandler: Send + Sync {
    fn can_handle(&self, event: &SessionEvent) -> bool;
    fn handle(&self, event: &SessionEvent) -> Result<()>;
    fn name(&self) -> &str;
}
```

**内置 Handlers**:

1. **SessionViewHandler** - 维护会话上下文视图
2. **KnowledgeExtractor** - 提取知识到 KnowledgeView
3. **DecisionTracker** - 记录决策到 DecisionView
4. **EmbeddingIndexer** - 生成和索引 embedding
5. **SummaryGenerator** - 生成摘要（替代当前的 ContextCompactor）

**Checkpoint 机制**:

```rust
pub struct Checkpoint {
    handler_name: String,
    last_sequence: i64,
    updated_at: DateTime<Utc>,
}
```

**处理流程**:

1. 启动时从 checkpoint 恢复上次位置
2. 订阅 EventStore 新事件
3. 对每个事件，调用所有匹配的 handlers
4. 成功后更新 checkpoint
5. 失败时记录到 `failed_events` 表，定期重试

### 3. 物化视图

#### 3.1 SessionView（会话上下文视图）

**用途**: 替代当前的 `process_history()`，为单次会话提供 token-budget-aware 的上下文

**存储结构**:

```rust
pub struct SessionView {
    session_key: String,
    recent_events: VecDeque<SessionEvent>,  // 最近 N 条，内存
    summary: Option<String>,                 // 压缩摘要，SQLite
    token_count: usize,
    last_updated: DateTime<Utc>,
}
```

**更新逻辑**:
- 新事件到达 → 追加到 `recent_events`
- 超过 token budget → 触发压缩，生成 summary
- 旧事件从内存移除，但保留在 EventStore

**查询接口**:

```rust
impl SessionView {
    pub fn get_context(
        session_key: &str, 
        token_budget: usize
    ) -> Result<Vec<ChatMessage>>;
}
```

#### 3.2 KnowledgeView（知识库视图）

**用途**: 替代当前的 Memory 系统，存储提取的结构化知识

**存储**: 保持当前的 Markdown 文件格式（`~/.gasket/memory/`），但数据来源改为从 EventStore 提取

**更新逻辑**:
- KnowledgeExtractor 分析事件内容
- 识别知识类型（决策、模式、偏好、概念）
- 生成或更新对应的 Memory 文件
- 更新 embedding 索引

**保留特性**:
- 六大场景分类（profile, active, knowledge, decisions, episodes, reference）
- 频率分层（hot, warm, cold, archived）
- 三阶段加载策略
- 人类可编辑

#### 3.3 DecisionView（决策历史视图）

**用途**: 专门追踪和查询决策历史

**存储**: SQLite 表 + 索引

```sql
CREATE TABLE decisions (
    id TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    decision_text TEXT NOT NULL,
    context TEXT,
    tags TEXT,  -- JSON array
    created_at TIMESTAMP NOT NULL,
    FOREIGN KEY (event_id) REFERENCES events(id)
);

CREATE INDEX idx_decisions_tags ON decisions(tags);
CREATE INDEX idx_decisions_created ON decisions(created_at);
```

**查询接口**:

```rust
impl DecisionView {
    pub fn query_by_tags(tags: &[String]) -> Result<Vec<Decision>>;
    pub fn query_by_time_range(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Decision>>;
}
```

### 4. ViewCoordinator（视图协调器）

**职责**: 提供统一的查询入口，根据查询意图自动路由到最优视图

**查询类型**:

```rust
pub enum HistoryQuery {
    // 会话上下文（最近对话）
    SessionContext { 
        session_key: String, 
        token_budget: usize 
    },
    
    // 语义搜索（跨会话知识）
    SemanticSearch { 
        query: String, 
        top_k: usize 
    },
    
    // 时间范围查询（原始事件）
    TimeRange { 
        session_key: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    
    // 决策历史
    Decisions { 
        tags: Vec<String>,
        limit: usize 
    },
}
```

**路由逻辑**:

```rust
impl ViewCoordinator {
    pub fn query(&self, query: HistoryQuery) -> Result<QueryResult> {
        match query {
            HistoryQuery::SessionContext { session_key, token_budget } 
                => self.session_view.get_context(&session_key, token_budget),
            
            HistoryQuery::SemanticSearch { query, top_k } 
                => self.knowledge_view.search(&query, top_k),
            
            HistoryQuery::TimeRange { session_key, start, end } 
                => self.event_store.get_events(&session_key, start, end),
            
            HistoryQuery::Decisions { tags, limit } 
                => self.decision_view.query_by_tags(&tags, limit),
        }
    }
}
```

**Agent Loop 集成**:

```rust
// 替代当前的 process_history() + MemoryManager
let context = coordinator.query(
    HistoryQuery::SessionContext { 
        session_key: session_key.clone(), 
        token_budget: 8000 
    }
)?;
```

## 生命周期管理

### 数据生命周期流

```
RawEvent (EventStore)
    ↓ [实时]
SessionView (内存 + 热存储)
    ↓ [会话结束后]
KnowledgeView (温存储，按频率分层)
    ↓ [90天未访问]
Archive (冷存储，只读)
```

### LifecycleManager 职责

**1. 会话归档**:
- 会话结束后，SessionView 从内存清除
- 有价值的事件提取到 KnowledgeView
- 原始事件保留在 EventStore

**2. 知识衰减**:
- 保持当前的频率衰减机制（hot → warm → cold → archived）
- 基于访问日志自动调整

**3. EventStore 压缩**:
- 定期压缩旧事件（合并、去重）
- 保留关键事件（用户消息、重要决策）
- 可选：超过 N 天的事件移到归档表

### 配置

```rust
pub struct LifecycleConfig {
    pub session_ttl_hours: u64,           // 会话在内存保留时间
    pub event_retention_days: u64,        // EventStore 保留时间
    pub archive_after_days: u64,          // 归档阈值
    pub compress_interval_hours: u64,     // 压缩间隔
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            session_ttl_hours: 24,
            event_retention_days: 365,
            archive_after_days: 90,
            compress_interval_hours: 24,
        }
    }
}
```


## 错误处理与一致性保证

### 一致性策略

**1. 写入一致性**:
- EventStore 写入成功 = 事务提交
- 视图更新失败不影响写入（最终一致性）
- 通过 checkpoint 机制保证视图最终追上

**2. 视图重建**:

```rust
impl ViewCoordinator {
    pub fn rebuild_view(&self, view_name: &str) -> Result<()> {
        // 1. 清空目标视图
        // 2. 重置 checkpoint 到 0
        // 3. 从 EventStore 重放所有事件
        // 4. 重建完成
    }
}
```

**3. 幂等性保证**:
- 所有 EventHandler 必须幂等
- 使用 `event.sequence` 去重
- 视图更新使用 upsert 语义

**4. 故障恢复**:
- MaterializationEngine 启动时从 checkpoint 恢复
- 处理失败的事件记录到 `failed_events` 表
- 定期重试失败事件（指数退避）

### 监控指标

```rust
pub struct MaterializationMetrics {
    pub event_lag: i64,              // 视图落后的事件数
    pub processing_latency_ms: f64,  // 处理延迟
    pub failed_events_count: usize,  // 失败事件数
    pub view_rebuild_count: usize,   // 重建次数
}
```

## 实现路线图

### 阶段 1: EventStore 重构（1-2 周）

**目标**: 收缩 EventStore 职责，添加事件订阅机制

- 添加 `sequence` 和 `schema_version` 字段
- 实现事件订阅接口
- 移除历史截断、语义搜索等高级功能
- 保持向后兼容

### 阶段 2: MaterializationEngine 实现（2-3 周）

**目标**: 构建物化引擎核心框架

- 实现 EventHandler trait 和注册机制
- 实现 Checkpoint 存储和恢复
- 实现基础的 SessionViewHandler
- 添加监控指标

### 阶段 3: 视图迁移（2-3 周）

**目标**: 将现有功能迁移到物化视图

- 实现 SessionView（替代 process_history）
- 迁移 KnowledgeView（复用现有 Memory 系统）
- 实现 DecisionView
- 实现 ViewCoordinator

### 阶段 4: Agent Loop 集成（1 周）

**目标**: 替换 Agent Loop 中的历史处理逻辑

- 用 ViewCoordinator 替代 process_history()
- 用 ViewCoordinator 替代 MemoryManager
- 更新测试

### 阶段 5: 生命周期管理（1-2 周）

**目标**: 实现完整的数据生命周期

- 实现 LifecycleManager
- 实现会话归档
- 实现 EventStore 压缩
- 添加配置选项

## 迁移策略

### 向后兼容

**数据迁移**:
1. 现有 EventStore 数据保持不变
2. 添加 `sequence` 列（自动生成）
3. 添加 `schema_version` 列（默认值 1）
4. 现有 Memory 文件保持不变

**API 兼容**:
- 保留现有的 `process_history()` 作为 deprecated wrapper
- 保留现有的 `MemoryManager` 接口，内部委托给 ViewCoordinator
- 提供迁移期的双写模式（同时写入旧系统和新系统）

### 灰度发布

1. **Phase 1**: 新系统只读，旧系统继续写入
2. **Phase 2**: 双写模式，新旧系统同时写入
3. **Phase 3**: 新系统主写，旧系统只读（验证）
4. **Phase 4**: 完全切换到新系统，移除旧代码

## 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 视图更新延迟影响用户体验 | 中 | 监控延迟指标，优化 handler 性能 |
| 视图重建耗时过长 | 中 | 增量重建，分批处理 |
| EventStore 存储膨胀 | 高 | 实现压缩和归档机制 |
| 知识提取准确性不足 | 中 | 人工审核 + 持续优化提取规则 |
| 迁移过程数据不一致 | 高 | 双写验证 + 回滚机制 |

## 总结

本设计通过**事件溯源 + CQRS** 模式彻底解决了历史记录模块的架构问题：

✅ **职责清晰**: EventStore = 事实记录，Views = 知识提取  
✅ **检索明确**: ViewCoordinator 统一路由，查询类型明确  
✅ **生命周期清晰**: 明确的数据流和归档策略  
✅ **边界清晰**: 写入单一路径，读取多样化  
✅ **可维护性**: 视图可重建，调试友好  
✅ **扩展性**: 新增查询需求只需添加新视图
