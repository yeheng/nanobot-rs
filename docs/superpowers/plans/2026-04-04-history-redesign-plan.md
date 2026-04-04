# 历史记录模块重新设计实现计划

> 基于规格: `docs/superpowers/specs/2026-04-04-history-redesign-design.md`

## 计划概述

**目标**: 将历史记录模块重构为事件溯源 + 物化视图（CQRS）架构

**总工期**: 8-10 周

**关键里程碑**:
1. EventStore 重构（2 周）
2. MaterializationEngine 实现（3 周）
3. 视图迁移（3 周）
4. Agent Loop 集成（1 周）
5. 生命周期管理（2 周）

---

## 阶段 1: EventStore 重构（1-2 周）

### 目标
收缩 EventStore 职责为纯事件日志，添加事件订阅机制

### 任务清单

#### 1.1 数据模型扩展
- [ ] 在 `types/src/session_event.rs` 添加 `sequence: i64` 字段
- [ ] 在 `types/src/session_event.rs` 添加 `schema_version: u32` 字段
- [ ] 更新 `SessionEvent` 构造函数，自动设置 `schema_version = 1`
- [ ] 在 `storage/src/event_store.rs` 的 SQLite schema 添加 `sequence` 列（自增）
- [ ] 在 `storage/src/event_store.rs` 的 SQLite schema 添加 `schema_version` 列（默认 1）

**文件**: 
- `gasket/types/src/session_event.rs`
- `gasket/storage/src/event_store.rs`

**验证**: 运行现有测试，确保向后兼容


#### 1.2 事件订阅机制
- [ ] 在 `storage/src/event_store.rs` 添加 `EventSubscriber` trait
- [ ] 实现 `subscribe()` 方法，支持注册回调
- [ ] 实现 `publish()` 内部方法，在 `append_event()` 后调用
- [ ] 使用 `tokio::sync::broadcast` 实现发布-订阅
- [ ] 添加订阅者管理（注册、取消注册）

**接口设计**:
```rust
pub trait EventSubscriber: Send + Sync {
    fn on_event(&self, event: &SessionEvent) -> Result<()>;
}

impl EventStore {
    pub fn subscribe(&self, subscriber: Arc<dyn EventSubscriber>) -> SubscriptionId;
    pub fn unsubscribe(&self, id: SubscriptionId);
}
```

**文件**: `gasket/storage/src/event_store.rs`

#### 1.3 事件重放接口
- [ ] 实现 `replay_events(from_sequence, to_sequence)` 方法
- [ ] 支持按序列号范围查询
- [ ] 支持按时间范围查询
- [ ] 添加批量读取优化（每次 1000 条）

**文件**: `gasket/storage/src/event_store.rs`

#### 1.4 移除冗余功能
- [ ] 标记 `process_history()` 为 deprecated（保留实现）
- [ ] 从 EventStore 移除语义搜索相关代码（保留 embedding 字段）
- [ ] 文档注释说明职责变更

**文件**: 
- `gasket/storage/src/processor.rs`
- `gasket/storage/src/event_store.rs`

### 验收标准
- ✅ 所有现有测试通过
- ✅ 新增字段有默认值，不破坏现有数据
- ✅ 事件订阅机制可以注册多个订阅者
- ✅ 事件重放可以按序列号和时间范围查询

---

## 阶段 2: MaterializationEngine 实现（2-3 周）

### 目标
构建物化引擎核心框架，支持事件处理和 checkpoint 管理

### 任务清单

#### 2.1 核心类型定义
- [ ] 创建 `storage/src/materialization/mod.rs`
- [ ] 定义 `EventHandler` trait
- [ ] 定义 `Checkpoint` 结构体
- [ ] 定义 `MaterializationEngine` 结构体
- [ ] 定义 `MaterializationMetrics` 结构体

**文件**: `gasket/storage/src/materialization/mod.rs`


#### 2.2 Checkpoint 存储
- [ ] 创建 SQLite 表 `materialization_checkpoints`
- [ ] 实现 `CheckpointStore` 结构体
- [ ] 实现 `save_checkpoint(handler_name, sequence)` 方法
- [ ] 实现 `load_checkpoint(handler_name)` 方法
- [ ] 实现 `reset_checkpoint(handler_name)` 方法

**Schema**:
```sql
CREATE TABLE materialization_checkpoints (
    handler_name TEXT PRIMARY KEY,
    last_sequence INTEGER NOT NULL,
    updated_at TIMESTAMP NOT NULL
);
```

**文件**: `gasket/storage/src/materialization/checkpoint.rs`

#### 2.3 MaterializationEngine 实现
- [ ] 实现 `MaterializationEngine::new()`
- [ ] 实现 `register_handler()` 方法
- [ ] 实现 `start()` 方法（启动事件处理循环）
- [ ] 实现 `stop()` 方法（优雅停止）
- [ ] 实现事件处理循环（从 checkpoint 恢复 → 订阅新事件）
- [ ] 实现错误处理和重试逻辑

**文件**: `gasket/storage/src/materialization/engine.rs`

#### 2.4 失败事件处理
- [ ] 创建 SQLite 表 `failed_events`
- [ ] 实现失败事件记录
- [ ] 实现定期重试机制（指数退避）
- [ ] 实现死信队列（超过最大重试次数）

**Schema**:
```sql
CREATE TABLE failed_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    handler_name TEXT NOT NULL,
    event_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    error_message TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_retry_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL
);
```

**文件**: `gasket/storage/src/materialization/failed_events.rs`


#### 2.5 基础 SessionViewHandler 实现
- [ ] 创建 `storage/src/materialization/handlers/session_view.rs`
- [ ] 实现 `SessionViewHandler` 结构体
- [ ] 实现 `EventHandler` trait
- [ ] 实现 `can_handle()` - 处理所有事件类型
- [ ] 实现 `handle()` - 追加事件到内存队列
- [ ] 实现基础的 token 计数和截断逻辑

**文件**: `gasket/storage/src/materialization/handlers/session_view.rs`

#### 2.6 监控指标
- [ ] 实现 `MaterializationMetrics` 收集
- [ ] 添加 `event_lag` 计算（最新事件序列号 - checkpoint）
- [ ] 添加 `processing_latency_ms` 统计
- [ ] 添加 `failed_events_count` 统计
- [ ] 集成到现有的 OpenTelemetry 系统

**文件**: `gasket/storage/src/materialization/metrics.rs`

### 验收标准
- ✅ MaterializationEngine 可以启动和停止
- ✅ 可以注册多个 EventHandler
- ✅ Checkpoint 机制正常工作（重启后从上次位置继续）
- ✅ 失败事件会被记录和重试
- ✅ SessionViewHandler 可以处理事件并维护内存队列

---

## 阶段 3: 视图迁移（2-3 周）

### 目标
实现三个物化视图并迁移现有功能

### 任务清单

#### 3.1 SessionView 完整实现
- [ ] 创建 `storage/src/views/session_view.rs`
- [ ] 实现 `SessionView` 结构体（内存 + SQLite 混合）
- [ ] 实现 `get_context(session_key, token_budget)` 方法
- [ ] 实现摘要生成逻辑（复用 ContextCompactor）
- [ ] 实现 token 预算管理
- [ ] 创建 SQLite 表存储摘要

**Schema**:
```sql
CREATE TABLE session_summaries (
    session_key TEXT PRIMARY KEY,
    summary TEXT NOT NULL,
    token_count INTEGER NOT NULL,
    last_event_sequence INTEGER NOT NULL,
    updated_at TIMESTAMP NOT NULL
);
```

**文件**: `gasket/storage/src/views/session_view.rs`


#### 3.2 KnowledgeView 迁移
- [ ] 创建 `storage/src/views/knowledge_view.rs`
- [ ] 实现 `KnowledgeExtractor` handler
- [ ] 复用现有的 Memory 文件系统（`storage/src/memory/`）
- [ ] 实现知识提取规则（决策、模式、偏好识别）
- [ ] 集成到 MaterializationEngine

**提取规则示例**:
- 用户明确表达偏好 → profile/preferences.md
- 做出技术选择并说明理由 → decisions/*.md
- 学习新概念或模式 → knowledge/*.md

**文件**: `gasket/storage/src/views/knowledge_view.rs`

#### 3.3 DecisionView 实现
- [ ] 创建 `storage/src/views/decision_view.rs`
- [ ] 创建 SQLite 表 `decisions`
- [ ] 实现 `DecisionTracker` handler
- [ ] 实现决策识别逻辑（关键词匹配 + 上下文分析）
- [ ] 实现 `query_by_tags()` 方法
- [ ] 实现 `query_by_time_range()` 方法

**Schema**:
```sql
CREATE TABLE decisions (
    id TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    decision_text TEXT NOT NULL,
    context TEXT,
    tags TEXT,
    created_at TIMESTAMP NOT NULL,
    FOREIGN KEY (event_id) REFERENCES events(id)
);
CREATE INDEX idx_decisions_tags ON decisions(tags);
CREATE INDEX idx_decisions_created ON decisions(created_at);
```

**文件**: `gasket/storage/src/views/decision_view.rs`


#### 3.4 ViewCoordinator 实现
- [ ] 创建 `storage/src/views/coordinator.rs`
- [ ] 定义 `HistoryQuery` 枚举
- [ ] 实现 `ViewCoordinator` 结构体
- [ ] 实现 `query()` 方法（路由逻辑）
- [ ] 实现查询结果统一封装

**文件**: `gasket/storage/src/views/coordinator.rs`

#### 3.5 EmbeddingIndexer Handler
- [ ] 创建 `storage/src/materialization/handlers/embedding_indexer.rs`
- [ ] 实现 `EmbeddingIndexer` handler
- [ ] 为新事件生成 embedding（如果启用 local-embedding）
- [ ] 更新 `memory_embeddings` 表
- [ ] 集成到 MaterializationEngine

**文件**: `gasket/storage/src/materialization/handlers/embedding_indexer.rs`

### 验收标准
- ✅ SessionView 可以返回 token-budget-aware 的上下文
- ✅ KnowledgeView 可以从事件中提取知识到 Memory 文件
- ✅ DecisionView 可以识别和存储决策
- ✅ ViewCoordinator 可以根据查询类型路由到正确的视图
- ✅ 所有视图都通过 MaterializationEngine 更新

---

## 阶段 4: Agent Loop 集成（1 周）

### 目标
替换 Agent Loop 中的历史处理逻辑

### 任务清单

#### 4.1 集成 ViewCoordinator
- [ ] 在 `engine/src/agent/loop_.rs` 中引入 `ViewCoordinator`
- [ ] 替换 `process_history()` 调用为 `coordinator.query(SessionContext)`
- [ ] 替换 `MemoryManager` 调用为 `coordinator.query(SemanticSearch)`
- [ ] 保留旧接口作为 deprecated wrapper（向后兼容）

**文件**: `gasket/engine/src/agent/loop_.rs`


#### 4.2 启动 MaterializationEngine
- [ ] 在 `cli/src/commands/agent.rs` 启动时初始化 MaterializationEngine
- [ ] 注册所有 handlers（SessionViewHandler, KnowledgeExtractor, DecisionTracker, EmbeddingIndexer）
- [ ] 订阅 EventStore 事件
- [ ] 在程序退出时优雅停止

**文件**: `gasket/cli/src/commands/agent.rs`

#### 4.3 更新测试
- [ ] 更新 `engine/tests/` 中的集成测试
- [ ] 添加 ViewCoordinator 的单元测试
- [ ] 添加端到端测试（写入事件 → 视图更新 → 查询）

**文件**: `gasket/engine/tests/`

### 验收标准
- ✅ Agent Loop 使用 ViewCoordinator 获取历史上下文
- ✅ MaterializationEngine 在后台运行
- ✅ 所有现有功能正常工作
- ✅ 测试全部通过

---

## 阶段 5: 生命周期管理（1-2 周）

### 目标
实现完整的数据生命周期管理

### 任务清单

#### 5.1 LifecycleManager 实现
- [ ] 创建 `storage/src/lifecycle/manager.rs`
- [ ] 实现 `LifecycleManager` 结构体
- [ ] 实现 `LifecycleConfig` 配置
- [ ] 实现定时任务调度（使用 tokio::time::interval）

**文件**: `gasket/storage/src/lifecycle/manager.rs`


#### 5.2 会话归档
- [ ] 实现会话结束检测（基于 session_idle_timeout）
- [ ] 实现 SessionView 内存清理
- [ ] 实现知识提取触发（会话结束时）
- [ ] 保留 EventStore 中的原始事件

**文件**: `gasket/storage/src/lifecycle/session_archiver.rs`

#### 5.3 知识衰减
- [ ] 实现频率衰减逻辑（hot → warm → cold → archived）
- [ ] 实现访问日志批量写入
- [ ] 实现定期衰减任务（每天运行）
- [ ] 保持 profile 场景永不衰减

**文件**: `gasket/storage/src/lifecycle/frequency_decay.rs`

#### 5.4 EventStore 压缩
- [ ] 实现旧事件压缩策略
- [ ] 保留关键事件（用户消息、重要决策）
- [ ] 实现归档表迁移（可选）
- [ ] 实现定期压缩任务（每周运行）

**Schema**:
```sql
CREATE TABLE archived_events (
    -- 与 events 表结构相同
    -- 用于存储超过保留期的事件
);
```

**文件**: `gasket/storage/src/lifecycle/event_compactor.rs`

#### 5.5 配置集成
- [ ] 在 `~/.gasket/config.yaml` 添加 lifecycle 配置段
- [ ] 实现配置加载
- [ ] 提供合理的默认值

**配置示例**:
```yaml
lifecycle:
  session_ttl_hours: 24
  event_retention_days: 365
  archive_after_days: 90
  compress_interval_hours: 24
```

**文件**: `gasket/engine/src/config/mod.rs`

### 验收标准
- ✅ 会话结束后自动归档
- ✅ 知识频率自动衰减
- ✅ EventStore 定期压缩
- ✅ 配置可以自定义生命周期参数

---


## 迁移策略

### 数据迁移脚本
- [ ] 创建 `cli/src/commands/migrate.rs`
- [ ] 实现 `migrate history-v2` 子命令
- [ ] 为现有事件生成 `sequence` 值
- [ ] 为现有事件设置 `schema_version = 1`
- [ ] 验证迁移结果

**文件**: `gasket/cli/src/commands/migrate.rs`

### 灰度发布计划

**Phase 1: 只读模式（1 周）**
- [ ] 部署新代码，MaterializationEngine 启动但不影响写入
- [ ] 监控视图更新延迟
- [ ] 验证视图数据正确性

**Phase 2: 双写模式（1 周）**
- [ ] 启用 ViewCoordinator 查询
- [ ] 保留旧的 process_history() 作为备份
- [ ] 对比新旧系统结果
- [ ] 修复发现的问题

**Phase 3: 主写模式（1 周）**
- [ ] ViewCoordinator 成为主查询路径
- [ ] 旧系统只读（用于验证）
- [ ] 监控性能和准确性

**Phase 4: 完全切换（1 周）**
- [ ] 移除旧代码
- [ ] 清理 deprecated 接口
- [ ] 更新文档

---

## 风险缓解

### 性能风险
**风险**: 视图更新延迟影响用户体验
**缓解**: 
- 监控 `event_lag` 指标，设置告警阈值（< 100 事件）
- 优化 handler 性能，使用批量处理
- 必要时增加 handler 并行度

### 数据一致性风险
**风险**: 迁移过程数据不一致
**缓解**:
- 双写验证期间对比结果
- 提供视图重建功能
- 保留回滚机制

### 存储膨胀风险
**风险**: EventStore 存储快速增长
**缓解**:
- 尽早实现压缩机制
- 监控存储使用量
- 提供手动归档工具

---


## 关键文件清单

### 新增文件
```
gasket/storage/src/materialization/
├── mod.rs                      # 模块入口
├── engine.rs                   # MaterializationEngine 核心
├── checkpoint.rs               # Checkpoint 存储
├── failed_events.rs            # 失败事件处理
├── metrics.rs                  # 监控指标
└── handlers/
    ├── mod.rs
    ├── session_view.rs         # SessionViewHandler
    ├── knowledge_extractor.rs  # KnowledgeExtractor
    ├── decision_tracker.rs     # DecisionTracker
    └── embedding_indexer.rs    # EmbeddingIndexer

gasket/storage/src/views/
├── mod.rs
├── session_view.rs             # SessionView 实现
├── knowledge_view.rs           # KnowledgeView 实现
├── decision_view.rs            # DecisionView 实现
└── coordinator.rs              # ViewCoordinator

gasket/storage/src/lifecycle/
├── mod.rs
├── manager.rs                  # LifecycleManager
├── session_archiver.rs         # 会话归档
├── frequency_decay.rs          # 频率衰减
└── event_compactor.rs          # 事件压缩

gasket/cli/src/commands/migrate.rs  # 数据迁移工具
```

### 修改文件
```
gasket/types/src/session_event.rs   # 添加 sequence, schema_version
gasket/storage/src/event_store.rs   # 添加订阅机制
gasket/storage/src/processor.rs     # 标记为 deprecated
gasket/engine/src/agent/loop_.rs    # 集成 ViewCoordinator
gasket/cli/src/commands/agent.rs    # 启动 MaterializationEngine
gasket/engine/src/config/mod.rs     # 添加 lifecycle 配置
```

---

## 测试策略

### 单元测试
- [ ] EventStore 订阅机制测试
- [ ] MaterializationEngine 核心逻辑测试
- [ ] Checkpoint 存储和恢复测试
- [ ] 各个 Handler 的处理逻辑测试
- [ ] ViewCoordinator 路由逻辑测试

### 集成测试
- [ ] 端到端流程测试（写入 → 物化 → 查询）
- [ ] 视图重建测试
- [ ] 失败重试测试
- [ ] 生命周期管理测试

### 性能测试
- [ ] 事件处理吞吐量测试（目标: > 1000 events/s）
- [ ] 视图查询延迟测试（目标: < 100ms）
- [ ] 内存使用测试
- [ ] 并发写入测试

---


## 文档更新

### 需要更新的文档
- [ ] `docs/architecture.md` - 更新架构图，添加物化视图层
- [ ] `docs/memory.md` - 更新为反映新的数据流
- [ ] `README.md` - 更新快速开始指南
- [ ] API 文档 - 更新 EventStore 和 ViewCoordinator 接口

---

## 时间线

| 阶段 | 任务 | 工期 | 依赖 |
|------|------|------|------|
| 1 | EventStore 重构 | 1-2 周 | 无 |
| 2 | MaterializationEngine 实现 | 2-3 周 | 阶段 1 |
| 3 | 视图迁移 | 2-3 周 | 阶段 2 |
| 4 | Agent Loop 集成 | 1 周 | 阶段 3 |
| 5 | 生命周期管理 | 1-2 周 | 阶段 4 |
| - | 灰度发布 | 4 周 | 阶段 5 |

**总工期**: 8-10 周（开发） + 4 周（灰度发布） = 12-14 周

---

## 成功指标

### 功能指标
- ✅ 所有现有功能正常工作
- ✅ 视图可以从 EventStore 重建
- ✅ 查询性能满足要求（< 100ms）
- ✅ 数据一致性验证通过

### 性能指标
- ✅ 事件处理延迟 < 100ms (p99)
- ✅ 视图更新延迟 < 100ms (p99)
- ✅ 查询响应时间 < 100ms (p95)
- ✅ 内存使用增长 < 20%

### 质量指标
- ✅ 测试覆盖率 > 80%
- ✅ 零数据丢失
- ✅ 零破坏性变更（向后兼容）

---

## 总结

本实现计划将历史记录模块重构为事件溯源 + 物化视图架构，分 5 个阶段完成：

1. **EventStore 重构** - 添加订阅机制，收缩职责
2. **MaterializationEngine** - 构建物化引擎核心
3. **视图迁移** - 实现三个专门化视图
4. **Agent Loop 集成** - 替换现有历史处理逻辑
5. **生命周期管理** - 实现完整的数据生命周期

通过灰度发布策略，确保平滑迁移，最小化风险。

