# 历史记录与记忆模块自动 Indexing 设计文档

**日期**: 2025-04-07
**状态**: 待评审
**作者**: Claude (via Linus Torvalds 审查模式)

---

## 1. 背景与目标

### 1.1 当前问题

Gasket 当前的历史记录和记忆系统存在以下问题：

1. **Embedding 生成手动化** - 需要显式调用才能生成向量表示
2. **语义搜索不完整** - 新写入的内容无法自动被搜索到
3. **索引策略缺失** - 没有统一的规则决定哪些内容需要 indexing

### 1.2 设计目标

- **自动性**: 内容写入后自动触发 indexing，无需人工干预
- **分层优先级**: 根据内容时效性采用不同处理策略
- **零侵入**: 不破坏现有 AgentLoop 的核心流程
- **可配置**: 支持通过元数据控制 indexing 行为

---

## 2. 架构设计

### 2.1 总体架构

```
┌─────────────────────────────────────────────────────────────────┐
│                        Indexing Pipeline                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐      ┌──────────────┐      ┌──────────────┐  │
│  │  Event Save  │─────▶│  IndexTask   │─────▶│   Queue      │  │
│  │  (P0 实时)   │      │  Generator   │      │  (优先级)     │  │
│  └──────────────┘      └──────────────┘      └──────────────┘  │
│                                                        │        │
│  ┌──────────────┐      ┌──────────────┐               │        │
│  │ Memory Write │─────▶│  FileWatcher │───────────────┘        │
│  │  (P0 实时)   │      │  (P1 增量)   │                        │
│  └──────────────┘      └──────────────┘                        │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Background Worker (Tokio Task)              │  │
│  │  ┌─────────┐    ┌─────────┐    ┌──────────────────────┐  │  │
│  │  │ Dequeue │───▶│Embedder │───▶│  SQLite Write        │  │  │
│  │  │ (P0→P2) │    │(fastembed)│   │  session_embeddings  │  │  │
│  │  └─────────┘    └─────────┘    │  memory_embeddings   │  │  │
│  │                                └──────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 核心组件

#### 2.2.1 IndexingService

```rust
pub struct IndexingService {
    /// SQLite 连接池
    pool: SqlitePool,
    /// 可选的 embedder（local-embedding 特性）
    embedder: Option<Arc<TextEmbedder>>,
    /// 优先级任务队列
    task_queue: Arc<PriorityQueue<IndexTask>>,
    /// 运行状态
    shutdown: Arc<AtomicBool>,
}

impl IndexingService {
    /// 提交 indexing 任务
    pub async fn submit(&self, task: IndexTask) -> Result<()>;

    /// 启动后台 worker
    pub fn start_background_worker(&self);

    /// 优雅关闭
    pub async fn shutdown(&self);
}
```

#### 2.2.2 IndexTask 定义

```rust
pub enum IndexTask {
    /// 实时任务：历史消息
    History {
        session_key: String,
        event_id: Uuid,
        content: String,
        priority: Priority,
        retry_count: u32,
    },
    /// 实时任务：记忆文件
    Memory {
        path: PathBuf,
        content: String,
        metadata: MemoryMeta,
        priority: Priority,
        retry_count: u32,
    },
    /// 批量任务
    Batch {
        tasks: Vec<IndexTask>,
        priority: Priority,
    },
}

pub enum Priority {
    P0, // 实时（新消息）
    P1, // 增量（会话恢复/文件修改）
    P2, // 全量（首次启动/批量导入）
}
```

---

## 3. 自动 Indexing 规则

### 3.1 历史记录 (session_events)

| 条件 | 行为 | 优先级 |
|------|------|--------|
| `event_type` 为 `UserMessage` 或 `AssistantMessage` | 生成 embedding | P0 |
| 内容长度 < 10 个字符 | 跳过 | - |
| 内容已存在 embedding | 跳过 | - |
| 批量导入历史 | 批量处理 | P1 |
| 系统重启后的积压 | 延迟处理 | P2 |

### 3.2 记忆文件 (memory files)

| 条件 | 行为 | 优先级 |
|------|------|--------|
| frontmatter 中 `index: false` | 跳过 | - |
| frontmatter 中 `index: true` 或缺失 | 生成 embedding | P0 |
| 文件修改时间 > 上次 index 时间 | 重新生成 | P1 |
| 首次扫描整个 memory 目录 | 批量处理 | P2 |

#### 3.2.1 Frontmatter 格式示例

```markdown
---
title: "项目架构笔记"
scenario: "gasket"
index: true        # 可选，默认为 true
frequency: "hot"   # hot/warm/cold
---

# 项目架构

内容...
```

---

## 4. 集成点设计

### 4.1 AgentLoop 集成

```rust
impl AgentLoop {
    pub async fn with_services(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
        pricing: Option<ModelPricing>,
        indexing_service: Arc<IndexingService>,  // 新增参数
    ) -> Result<Self, AgentError> {
        // 启动后台 worker
        indexing_service.start_background_worker();
        // ... 现有逻辑
    }

    /// 保存事件并触发 indexing（非阻塞）
    async fn save_and_index(&self, event: SessionEvent) {
        // 1. 保存到数据库（阻塞，确保数据安全）
        self.context.save_event(&event).await?;

        // 2. 提交 indexing 任务（非阻塞）
        if should_index(&event) {
            if let Some(ref svc) = self.indexing_service {
                let _ = svc.submit(IndexTask::History {
                    session_key: event.session_key.clone(),
                    event_id: event.id,
                    content: event.content.clone(),
                    priority: Priority::P0,
                    retry_count: 0,
                }).await;
            }
        }
    }
}

/// 判断是否需要 indexing
fn should_index(event: &SessionEvent) -> bool {
    match event.event_type {
        EventType::UserMessage | EventType::AssistantMessage => {
            event.content.len() >= 10
        }
        _ => false,
    }
}
```

### 4.2 MemoryStore 集成

```rust
impl MemoryStore {
    /// 写入记忆文件并触发 indexing
    pub async fn write_memory(
        &self,
        path: &Path,
        content: &str,
    ) -> Result<()> {
        // 1. 写入文件
        tokio::fs::write(path, content).await?;

        // 2. 解析 frontmatter
        let meta = parse_frontmatter(content)?;

        // 3. 提交 indexing 任务
        if meta.index != Some(false) {
            self.indexing.submit(IndexTask::Memory {
                path: path.to_path_buf(),
                content: content.to_string(),
                metadata: meta,
                priority: Priority::P0,
                retry_count: 0,
            }).await?;
        }

        Ok(())
    }
}
```

---

## 5. 后台 Worker 设计

### 5.1 处理流程

```rust
async fn background_worker(
    queue: Arc<PriorityQueue<IndexTask>>,
    embedder: Option<Arc<TextEmbedder>>,
    pool: SqlitePool,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        // 1. 获取任务（带超时，以便检查 shutdown）
        let task = match tokio::time::timeout(
            Duration::from_secs(1),
            queue.pop()
        ).await {
            Ok(Some(task)) => task,
            Ok(None) => continue,
            Err(_) => continue, // timeout
        };

        // 2. 处理任务
        let result = process_task(&task, &embedder, &pool).await;

        // 3. 处理结果
        match result {
            Ok(_) => metrics.tasks_completed.inc(),
            Err(e) => handle_failure(task, e, &queue).await,
        }
    }
}

async fn process_task(
    task: &IndexTask,
    embedder: &Option<Arc<TextEmbedder>>,
    pool: &SqlitePool,
) -> Result<(), IndexError> {
    let embedder = embedder.as_ref()
        .ok_or(IndexError::NoEmbedder)?;

    match task {
        IndexTask::History { event_id, content, .. } => {
            let embedding = embedder.embed(content).await?;
            save_history_embedding(pool, event_id, &embedding).await?;
        }
        IndexTask::Memory { path, content, .. } => {
            let embedding = embedder.embed(content).await?;
            save_memory_embedding(pool, path, &embedding).await?;
        }
        IndexTask::Batch { tasks, .. } => {
            for task in tasks {
                process_task(task, embedder, pool).await?;
            }
        }
    }

    Ok(())
}
```

### 5.2 错误处理与重试

```rust
async fn handle_failure(
    task: IndexTask,
    error: IndexError,
    queue: &PriorityQueue<IndexTask>,
) {
    metrics.tasks_failed.inc();

    if task.retry_count() < 3 {
        // 降级处理
        let mut retry = task;
        retry.set_priority(Priority::P2);
        retry.increment_retry();

        tokio::time::sleep(Duration::from_secs(2_u64.pow(retry.retry_count()))).await;
        queue.push(retry).await.ok();
    } else {
        tracing::error!("Indexing failed after 3 retries: {:?}", error);
    }
}
```

---

## 6. 存储设计

### 6.1 保持现有表结构

**session_embeddings** - 历史记录 embedding
```sql
CREATE TABLE IF NOT EXISTS session_embeddings (
    message_id  TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    embedding   BLOB NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_session_embeddings_session_key
    ON session_embeddings(session_key);
```

**memory_embeddings** - 记忆文件 embedding
```sql
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_path   TEXT PRIMARY KEY,
    scenario      TEXT NOT NULL,
    tags          TEXT,
    frequency     TEXT NOT NULL DEFAULT 'warm',
    embedding     BLOB NOT NULL,
    token_count   INTEGER NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_mem_emb_scenario
    ON memory_embeddings(scenario);
```

### 6.2 新增元数据表

```sql
-- 记录 indexing 状态
CREATE TABLE IF NOT EXISTS indexing_status (
    id            TEXT PRIMARY KEY,
    source_type   TEXT NOT NULL,  -- 'history' | 'memory'
    source_id     TEXT NOT NULL,  -- event_id 或 file_path
    status        TEXT NOT NULL,  -- 'pending' | 'indexed' | 'failed'
    error_msg     TEXT,
    retry_count   INTEGER DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_indexing_status_source
    ON indexing_status(source_type, status);
```

---

## 7. 性能考虑

### 7.1 并发控制

- **Worker 数量**: 单 worker 顺序处理（SQLite 限制）
- **批量处理**: Batch 任务内部串行，避免事务冲突
- **优先级抢占**: P0 任务插队到队列头部

### 7.2 资源限制

```rust
pub struct IndexingLimits {
    /// 最大并发 embedding 请求
    pub max_concurrent_embeds: usize,
    /// 批量处理大小
    pub batch_size: usize,
    /// 队列最大深度
    pub max_queue_depth: usize,
}

impl Default for IndexingLimits {
    fn default() -> Self {
        Self {
            max_concurrent_embeds: 1,  // fastembed 内部已优化
            batch_size: 10,
            max_queue_depth: 10000,
        }
    }
}
```

---

## 8. 监控与可观测性

### 8.1 指标定义

```rust
pub struct IndexingMetrics {
    /// 提交的任务数
    pub tasks_submitted: Counter,
    /// 完成的任务数
    pub tasks_completed: Counter,
    /// 失败的任务数
    pub tasks_failed: Counter,
    /// Embedding 生成延迟
    pub embedding_latency: Histogram,
    /// 队列深度
    pub queue_depth: Gauge,
    /// 重试次数分布
    pub retry_distribution: Histogram,
}
```

### 8.2 日志规范

```rust
// 任务提交
info!(target: "indexing", "Task submitted: {:?} (priority: {:?})", task_id, priority);

// 任务完成
debug!(target: "indexing", "Task completed: {:?} (latency: {:?})", task_id, latency);

// 任务失败
warn!(target: "indexing", "Task failed: {:?} (error: {}, retry: {})",
      task_id, error, retry_count);

// 队列状态
trace!(target: "indexing", "Queue depth: {} (P0: {}, P1: {}, P2: {})",
       total, p0_count, p1_count, p2_count);
```

---

## 9. 测试策略

### 9.1 单元测试

```rust
#[tokio::test]
async fn test_should_index_filter() {
    let user_msg = create_event(EventType::UserMessage, "Hello world");
    assert!(should_index(&user_msg));

    let tool_msg = create_event(EventType::ToolCall { ... }, "result");
    assert!(!should_index(&tool_msg));

    let short_msg = create_event(EventType::UserMessage, "Hi");
    assert!(!should_index(&short_msg));
}

#[tokio::test]
async fn test_priority_queue_ordering() {
    let queue = PriorityQueue::new();

    queue.push(IndexTask::History { priority: P2, ... }).await;
    queue.push(IndexTask::History { priority: P0, ... }).await;
    queue.push(IndexTask::History { priority: P1, ... }).await;

    assert_eq!(queue.pop().await.unwrap().priority(), P0);
    assert_eq!(queue.pop().await.unwrap().priority(), P1);
    assert_eq!(queue.pop().await.unwrap().priority(), P2);
}
```

### 9.2 集成测试

```rust
#[tokio::test]
async fn test_end_to_end_indexing() {
    let (service, pool) = setup_test_indexing_service().await;

    // 启动 worker
    service.start_background_worker();

    // 提交任务
    service.submit(IndexTask::History { ... }).await.unwrap();

    // 等待处理
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 验证结果
    let embedding = sqlx::query("SELECT embedding FROM session_embeddings WHERE message_id = ?")
        .bind(event_id)
        .fetch_one(&pool)
        .await;

    assert!(embedding.is_ok());
}
```

---

## 10. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| fastembed 加载慢 | P0 任务延迟 | 懒加载 + 预加载选项 |
| SQLite 写入冲突 | 索引失败 | 单 worker + 重试机制 |
| 队列积压 | 内存溢出 | 队列深度限制 + 降级策略 |
| 嵌入模型失败 | 所有 indexing 失败 | graceful degradation |

---

## 11. 验收标准

- [ ] 新消息写入后 1 秒内自动 indexing（P0 任务）
- [ ] 记忆文件保存后自动 indexing（遵守 frontmatter 规则）
- [ ] 批量导入支持 P1/P2 优先级处理
- [ ] 失败任务自动重试（最多 3 次）
- [ ] 支持 `index: false` 跳过 indexing
- [ ] 单测覆盖率 > 80%
- [ ] 集成测试覆盖核心流程

---

## 12. 附录

### 12.1 相关代码文件

- `engine/src/agent/loop_.rs` - AgentLoop 核心
- `engine/src/agent/context.rs` - Context 管理
- `storage/src/lib.rs` - SQLite 存储
- `storage/src/memory.rs` - 记忆模块

### 12.2 依赖项

- `fastembed` (optional, local-embedding feature)
- `sqlx` - SQLite 异步访问
- `tokio` - 异步运行时
- `uuid` - UUID 生成

---

**审批记录**:
- 设计评审: 待进行
- 实现计划: 待创建

---

## 13. 修订记录 (Post-Review Updates)

### 13.1 MemoryMeta Schema Update

**问题**: 现有 `MemoryMeta` 缺少 `index` 字段

**修复**: 在 `storage/src/memory/types.rs` 中添加:

```rust
pub struct MemoryMeta {
    // ... existing fields ...
    
    /// Whether to index this memory for search (default: true)
    #[serde(default = "default_true")]
    pub index: bool,
}

fn default_true() -> bool { true }
```

### 13.2 Duplicate Prevention Strategy

**问题**: 重复 indexing 防止机制不明确

**修复**: 

1. **唯一约束**:
```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_embeddings_message_id 
    ON session_embeddings(message_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_embeddings_path 
    ON memory_embeddings(memory_path);
```

2. **Upsert 策略**:
```rust
async fn save_history_embedding(pool: &SqlitePool, event_id: Uuid, embedding: &[f32]) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO session_embeddings (message_id, session_key, embedding, created_at) 
         VALUES ($1, $2, $3, datetime('now'))"
    )
    .bind(event_id.to_string())
    .bind(session_key)
    .bind(embedding_bytes)
    .execute(pool)
    .await?;
    Ok(())
}
```

3. **Pre-check 优化** (避免不必要的 embedding 计算):
```rust
async fn should_skip_indexing(pool: &SqlitePool, event_id: &Uuid) -> Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM session_embeddings WHERE message_id = $1)"
    )
    .bind(event_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(exists)
}
```

### 13.3 AgentLoop Integration - Revised

**问题**: 原设计与现有 `with_services` 模式冲突

**修复**: `IndexingService` 在 `with_services` 内部构造，而非外部传入:

```rust
impl AgentLoop {
    async fn with_services(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
        pricing: Option<ModelPricing>,
    ) -> Result<Self, AgentError> {
        // 构造 IndexingService（内部使用 memory_store 的 pool）
        let indexing_service = Arc::new(
            IndexingService::new(memory_store.sqlite_store().pool()).await?
        );
        indexing_service.start_background_worker();
        
        // ... rest of initialization
        
        Ok(Self {
            // ... other fields
            indexing_service: Some(indexing_service),
        })
    }
}
```

### 13.4 Task Persistence on Shutdown

**问题**: 非阻塞提交可能丢失任务

**修复策略** (选择其一):

**方案 A - 持久化队列** (推荐用于生产):
```rust
pub struct PersistentIndexingQueue {
    pool: SqlitePool,
}

impl PersistentIndexingQueue {
    pub async fn submit(&self, task: IndexTask) -> Result<()> {
        sqlx::query(
            "INSERT INTO indexing_queue (task_data, priority, created_at) 
             VALUES ($1, $2, datetime('now'))"
        )
        .bind(serde_json::to_string(&task)?)
        .bind(task.priority() as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    
    pub async fn pop(&self) -> Result<Option<IndexTask>> {
        // 使用事务 + DELETE RETURNING 原子弹出
    }
}
```

**方案 B - 优雅关闭** (简单场景):
```rust
impl IndexingService {
    pub async fn graceful_shutdown(&self, timeout: Duration) {
        // 1. 停止接受新任务
        self.shutdown.store(true, Ordering::Relaxed);
        
        // 2. 等待队列处理完成或超时
        tokio::time::timeout(timeout, async {
            while self.queue.depth() > 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await.ok();
        
        // 3. 记录未完成的任务数
        let remaining = self.queue.depth();
        if remaining > 0 {
            warn!("{} indexing tasks dropped on shutdown", remaining);
        }
    }
}
```

### 13.5 PriorityQueue Implementation

**实现选择**:

```rust
/// 使用多个 tokio mpsc channel 实现优先级队列
pub struct PriorityQueue<T> {
    p0_tx: mpsc::Sender<T>,
    p0_rx: mpsc::Receiver<T>,
    p1_tx: mpsc::Sender<T>,
    p1_rx: mpsc::Receiver<T>,
    p2_tx: mpsc::Sender<T>,
    p2_rx: mpsc::Receiver<T>,
    max_depth: usize,
}

impl<T> PriorityQueue<T> {
    pub fn new(max_depth: usize) -> Self {
        let (p0_tx, p0_rx) = mpsc::channel(max_depth / 3);
        let (p1_tx, p1_rx) = mpsc::channel(max_depth / 3);
        let (p2_tx, p2_rx) = mpsc::channel(max_depth / 3);
        Self { p0_tx, p0_rx, p1_tx, p1_rx, p2_tx, p2_rx, max_depth }
    }
    
    pub async fn push(&self, task: T, priority: Priority) -> Result<(), QueueError> {
        let tx = match priority {
            Priority::P0 => &self.p0_tx,
            Priority::P1 => &self.p1_tx,
            Priority::P2 => &self.p2_tx,
        };
        
        match tx.try_send(task) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics.queue_dropped.inc();
                Err(QueueError::Full)
            }
            Err(e) => Err(QueueError::SendError(e.to_string())),
        }
    }
    
    pub async fn pop(&mut self) -> Option<T> {
        // 优先级: P0 > P1 > P2
        tokio::select! {
            biased; // 按顺序检查
            task = self.p0_rx.recv() => task,
            task = self.p1_rx.recv() => task,
            task = self.p2_rx.recv() => task,
        }
    }
    
    pub fn depth(&self) -> usize {
        self.p0_tx.len() + self.p1_tx.len() + self.p2_tx.len()
    }
}
```

### 13.6 Memory File Indexing - Body Only

**问题**: 不应该 embedding frontmatter

**修复**:

```rust
use crate::storage::memory::frontmatter::extract_body;

async fn index_memory_file(path: &Path, content: &str) -> Result<()> {
    // 只 embedding 正文，排除 YAML frontmatter
    let body = extract_body(content);
    
    if body.len() < 10 {
        debug!("Skipping short memory body: {}", path.display());
        return Ok(());
    }
    
    let embedding = embedder.embed(body).await?;
    save_memory_embedding(pool, path, &embedding).await?;
    
    Ok(())
}
```

### 13.7 Batch Task Transaction

**问题**: Batch 任务缺少事务边界

**修复**:

```rust
async fn process_batch(
    tasks: &[IndexTask],
    embedder: &TextEmbedder,
    pool: &SqlitePool,
) -> Result<BatchResult> {
    let mut tx = pool.begin().await?;
    let mut success_count = 0;
    let mut failed_tasks = Vec::new();
    
    for task in tasks {
        match process_task_in_tx(task, embedder, &mut tx).await {
            Ok(_) => success_count += 1,
            Err(e) => {
                failed_tasks.push((task.id(), e));
                // 继续处理其他任务，不中断
            }
        }
    }
    
    tx.commit().await?;
    
    Ok(BatchResult {
        total: tasks.len(),
        success: success_count,
        failed: failed_tasks,
    })
}
```

### 13.8 AutoIndexHandler Relationship

**决策**: 替换现有 `AutoIndexHandler`

现有 `storage/src/memory/watcher.rs` 中的 `AutoIndexHandler` 功能将被 `IndexingService` 替代：

| 功能 | AutoIndexHandler (旧) | IndexingService (新) |
|------|----------------------|---------------------|
| 触发方式 | 文件 watcher 回调 | `MemoryStore::write_memory` 显式调用 |
| 队列 | 无，同步处理 | 优先级队列 |
| 持久化 | mtime 检查 | 唯一约束 + upsert |
| 错误处理 | 简单日志 | 重试机制 |
| 批处理 | 无 | 支持 |

**迁移计划**:
1. 实现 `IndexingService`
2. 在 `MemoryStore` 中集成 `IndexingService::submit`
3. 移除 `AutoIndexHandler` 的 indexing 逻辑
4. 保留文件 watcher 用于触发 reload（非 indexing）

