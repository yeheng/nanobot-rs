# 自动 Indexing 功能实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use @superpowers:subagent-driven-development (recommended) or @superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现历史记录和记忆模块的自动 indexing/embedding 功能，支持 P0/P1/P2 优先级队列和后台异步处理。

**Architecture:** 新增 `IndexingService` 作为独立服务，通过优先级队列管理 indexing 任务，后台 worker 异步处理 embedding 生成。与 `AgentLoop` 和 `MemoryStore` 非阻塞集成。

**Tech Stack:** Rust, tokio, sqlx, fastembed (local-embedding feature)

---

## 文件结构

### 新建文件

| 文件 | 职责 |
|------|------|
| `engine/src/indexing/mod.rs` | IndexingService 模块入口 |
| `engine/src/indexing/service.rs` | 核心服务实现 |
| `engine/src/indexing/queue.rs` | 优先级队列实现 |
| `engine/src/indexing/task.rs` | IndexTask 定义和序列化 |
| `engine/src/indexing/worker.rs` | 后台 worker 实现 |
| `engine/src/indexing/metrics.rs` | 指标收集 |
| `engine/tests/indexing_integration.rs` | 集成测试 |

### 修改文件

| 文件 | 变更内容 |
|------|---------|
| `storage/src/memory/types.rs` | 添加 `MemoryMeta.index` 字段 |
| `engine/src/agent/loop_.rs` | 集成 IndexingService，修改 `save_event` |
| `engine/src/agent/context.rs` | 添加 `save_and_index` 方法 |
| `engine/src/lib.rs` | 导出 indexing 模块 |
| `storage/src/lib.rs` | 添加唯一约束索引 |

---

## Phase 1: 基础类型和队列

### Task 1: 添加 MemoryMeta.index 字段

**Files:**
- Modify: `storage/src/memory/types.rs`

**上下文:**
现有的 `MemoryMeta` 结构体需要支持 `index: false` 来跳过 indexing。

- [ ] **Step 1: 修改 MemoryMeta 结构体**

在 `storage/src/memory/types.rs` 中，找到 `MemoryMeta` 结构体（约第199行），添加 `index` 字段：

```rust
pub struct MemoryMeta {
    /// ... existing fields ...
    
    /// Whether to index this memory for search (default: true)
    #[serde(default = "default_true")]
    pub index: bool,
}

fn default_true() -> bool { true }
```

- [ ] **Step 2: 更新 Default 实现**

在同一个文件的 `Default` impl 中，添加 `index: true`：

```rust
impl Default for MemoryMeta {
    fn default() -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            // ... existing fields ...
            index: true,  // 新增
        }
    }
}
```

- [ ] **Step 3: 运行测试确保编译通过**

```bash
cargo check --package gasket-storage
```

Expected: 无编译错误

- [ ] **Step 4: Commit**

```bash
git add storage/src/memory/types.rs
git commit -m "feat(storage): add index field to MemoryMeta for controlling auto-indexing

- Add `index: bool` field with serde default true
- Allows users to skip indexing via frontmatter: index: false"
```

---

### Task 2: 创建 PriorityQueue 实现

**Files:**
- Create: `engine/src/indexing/queue.rs`
- Modify: `engine/src/indexing/mod.rs` (创建空文件)

**设计:**
使用三个独立的 tokio mpsc channel 实现优先级队列，`tokio::select! biased` 确保 P0 优先。

- [ ] **Step 1: 创建 indexing 模块目录和 mod.rs**

```bash
mkdir -p engine/src/indexing
touch engine/src/indexing/mod.rs
```

- [ ] **Step 2: 编写 queue.rs**

创建 `engine/src/indexing/queue.rs`：

```rust
use tokio::sync::mpsc;
use std::sync::Arc;

/// Priority levels for indexing tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    P0 = 0, // Real-time (new messages)
    P1 = 1, // Incremental (session restore)
    P2 = 2, // Batch (initial scan)
}

impl Priority {
    pub fn as_usize(&self) -> usize {
        *self as usize
    }
}

/// Priority queue using multiple tokio channels
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
        let per_queue = max_depth / 3;
        let (p0_tx, p0_rx) = mpsc::channel(per_queue);
        let (p1_tx, p1_rx) = mpsc::channel(per_queue);
        let (p2_tx, p2_rx) = mpsc::channel(per_queue);
        
        Self {
            p0_tx, p0_rx,
            p1_tx, p1_rx,
            p2_tx, p2_rx,
            max_depth,
        }
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
                tracing::warn!("Priority queue full for {:?}", priority);
                Err(QueueError::Full)
            }
            Err(e) => Err(QueueError::SendError(e.to_string())),
        }
    }
    
    pub async fn pop(&mut self) -> Option<T> {
        tokio::select! {
            biased; // Check P0 first, then P1, then P2
            task = self.p0_rx.recv() => task,
            task = self.p1_rx.recv() => task,
            task = self.p2_rx.recv() => task,
        }
    }
    
    pub fn depth(&self) -> usize {
        self.p0_tx.len() + self.p1_tx.len() + self.p2_tx.len()
    }
    
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Queue is full")]
    Full,
    #[error("Send error: {0}")]
    SendError(String),
}
```

- [ ] **Step 3: 编写 mod.rs**

创建 `engine/src/indexing/mod.rs`：

```rust
pub mod queue;

pub use queue::{PriorityQueue, Priority, QueueError};
```

- [ ] **Step 4: 添加单元测试**

在 `engine/src/indexing/queue.rs` 末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_priority_queue_ordering() {
        let mut queue = PriorityQueue::new(100);
        
        queue.push("p2_task", Priority::P2).await.unwrap();
        queue.push("p0_task", Priority::P0).await.unwrap();
        queue.push("p1_task", Priority::P1).await.unwrap();
        
        // Should pop in priority order: P0, P1, P2
        assert_eq!(queue.pop().await, Some("p0_task"));
        assert_eq!(queue.pop().await, Some("p1_task"));
        assert_eq!(queue.pop().await, Some("p2_task"));
    }
    
    #[tokio::test]
    async fn test_queue_full() {
        let queue = PriorityQueue::new(3); // 1 per priority
        
        queue.push(1, Priority::P0).await.unwrap();
        queue.push(2, Priority::P0).await.unwrap(); // Should fail - P0 queue full
        
        assert!(matches!(
            queue.push(2, Priority::P0).await,
            Err(QueueError::Full)
        ));
    }
}
```

- [ ] **Step 5: 运行测试**

```bash
cargo test --package gasket-engine indexing::queue::tests
```

Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add engine/src/indexing/
git commit -m "feat(indexing): add PriorityQueue with P0/P1/P2 levels

- Multi-channel implementation with biased select
- Non-blocking push with backpressure
- Unit tests for ordering and capacity"
```

---

### Task 3: 创建 IndexTask 定义

**Files:**
- Create: `engine/src/indexing/task.rs`
- Modify: `engine/src/indexing/mod.rs`

**设计:**
定义 IndexTask 枚举，支持 History 和 Memory 两种任务类型，可序列化用于持久化队列。

- [ ] **Step 1: 编写 task.rs**

创建 `engine/src/indexing/task.rs`：

```rust
use std::path::PathBuf;
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use crate::indexing::queue::Priority;

/// Indexing task types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexTask {
    /// Real-time history event indexing
    History {
        session_key: String,
        event_id: Uuid,
        content: String,
        #[serde(skip)]
        priority: Priority,
        retry_count: u32,
    },
    
    /// Memory file indexing
    Memory {
        path: PathBuf,
        content: String,
        #[serde(skip)]
        priority: Priority,
        retry_count: u32,
    },
    
    /// Batch processing multiple tasks
    Batch {
        tasks: Vec<IndexTask>,
        #[serde(skip)]
        priority: Priority,
    },
}

impl IndexTask {
    /// Get the priority level
    pub fn priority(&self) -> Priority {
        match self {
            IndexTask::History { priority, .. } => *priority,
            IndexTask::Memory { priority, .. } => *priority,
            IndexTask::Batch { priority, .. } => *priority,
        }
    }
    
    /// Get retry count
    pub fn retry_count(&self) -> u32 {
        match self {
            IndexTask::History { retry_count, .. } => *retry_count,
            IndexTask::Memory { retry_count, .. } => *retry_count,
            IndexTask::Batch { .. } => 0, // Batches don't retry as a unit
        }
    }
    
    /// Increment retry count, returning a new task
    pub fn with_incremented_retry(&self) -> Self {
        match self.clone() {
            IndexTask::History { session_key, event_id, content, priority, retry_count } => {
                IndexTask::History {
                    session_key,
                    event_id,
                    content,
                    priority,
                    retry_count: retry_count + 1,
                }
            }
            IndexTask::Memory { path, content, priority, retry_count } => {
                IndexTask::Memory {
                    path,
                    content,
                    priority,
                    retry_count: retry_count + 1,
                }
            }
            IndexTask::Batch { tasks, priority } => {
                IndexTask::Batch { tasks, priority }
            }
        }
    }
    
    /// Create a new History task with P0 priority
    pub fn history(session_key: impl Into<String>, event_id: Uuid, content: impl Into<String>) -> Self {
        IndexTask::History {
            session_key: session_key.into(),
            event_id,
            content: content.into(),
            priority: Priority::P0,
            retry_count: 0,
        }
    }
    
    /// Create a new Memory task with P0 priority
    pub fn memory(path: PathBuf, content: impl Into<String>) -> Self {
        IndexTask::Memory {
            path,
            content: content.into(),
            priority: Priority::P0,
            retry_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_task_creation() {
        let task = IndexTask::history("session:123", Uuid::new_v4(), "test content");
        assert!(matches!(task, IndexTask::History { .. }));
        assert_eq!(task.priority(), Priority::P0);
        assert_eq!(task.retry_count(), 0);
    }
    
    #[test]
    fn test_retry_increment() {
        let task = IndexTask::history("session:123", Uuid::new_v4(), "test");
        let retried = task.with_incremented_retry();
        assert_eq!(retried.retry_count(), 1);
    }
    
    #[test]
    fn test_serialization() {
        let task = IndexTask::history("session:123", Uuid::new_v4(), "test content");
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: IndexTask = serde_json::from_str(&json).unwrap();
        
        match deserialized {
            IndexTask::History { session_key, content, .. } => {
                assert_eq!(session_key, "session:123");
                assert_eq!(content, "test content");
            }
            _ => panic!("Wrong task type"),
        }
    }
}
```

- [ ] **Step 2: 更新 mod.rs**

修改 `engine/src/indexing/mod.rs`：

```rust
pub mod queue;
pub mod task;

pub use queue::{PriorityQueue, Priority, QueueError};
pub use task::IndexTask;
```

- [ ] **Step 3: 运行测试**

```bash
cargo test --package gasket-engine indexing::task::tests
```

Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add engine/src/indexing/
git commit -m "feat(indexing): add IndexTask definition with serialization

- History and Memory task types
- Retry count tracking
- Serde support for persistence"
```

---

## Phase 2: IndexingService 核心

### Task 4: 创建 IndexingService 骨架

**Files:**
- Create: `engine/src/indexing/service.rs`
- Modify: `engine/src/indexing/mod.rs`

**设计:**
创建 `IndexingService` 结构体，包含 queue、embedder 和 shutdown 标志。先实现基础 API，后续添加 worker。

- [ ] **Step 1: 编写 service.rs 骨架**

创建 `engine/src/indexing/service.rs`：

```rust
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::indexing::{PriorityQueue, IndexTask, Priority};

#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;

pub struct IndexingService {
    pool: SqlitePool,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    queue: Arc<PriorityQueue<IndexTask>>,
    shutdown: Arc<AtomicBool>,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("No embedder available")]
    NoEmbedder,
    #[error("Embedding failed: {0}")]
    Embedding(String),
    #[error("Queue error: {0}")]
    Queue(#[from] crate::indexing::QueueError),
}

impl IndexingService {
    pub async fn new(pool: SqlitePool) -> Result<Self, IndexError> {
        #[cfg(feature = "local-embedding")]
        let embedder = match TextEmbedder::new(Default::default()).await {
            Ok(e) => {
                info!("TextEmbedder initialized successfully");
                Some(Arc::new(e))
            }
            Err(e) => {
                warn!("Failed to initialize TextEmbedder: {}", e);
                None
            }
        };
        
        #[cfg(not(feature = "local-embedding"))]
        let embedder = None::<()>;
        
        let queue = Arc::new(PriorityQueue::new(10000)); // max 10k tasks
        
        Ok(Self {
            pool,
            #[cfg(feature = "local-embedding")]
            embedder,
            queue,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }
    
    pub async fn submit(&self, task: IndexTask) -> Result<(), IndexError> {
        if self.shutdown.load(Ordering::Relaxed) {
            warn!("IndexingService is shutting down, rejecting new task");
            return Ok(()); // Graceful degradation
        }
        
        let priority = task.priority();
        self.queue.push(task, priority).await?;
        
        tracing::debug!("Indexing task submitted with priority {:?}", priority);
        Ok(())
    }
    
    pub fn start_background_worker(&self) {
        // TODO: Implement in Task 5
        info!("Background worker started (TODO)");
    }
    
    pub async fn graceful_shutdown(&self, timeout: std::time::Duration) {
        info!("IndexingService shutdown requested");
        self.shutdown.store(true, Ordering::Relaxed);
        
        // Wait for queue to drain or timeout
        let start = std::time::Instant::now();
        while self.queue.depth() > 0 && start.elapsed() < timeout {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        let remaining = self.queue.depth();
        if remaining > 0 {
            warn!("{} indexing tasks dropped on shutdown", remaining);
        } else {
            info!("All indexing tasks completed");
        }
    }
    
    pub fn queue_depth(&self) -> usize {
        self.queue.depth()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    
    async fn create_test_service() -> IndexingService {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        IndexingService::new(pool).await.unwrap()
    }
    
    #[tokio::test]
    async fn test_submit_and_shutdown() {
        let service = create_test_service().await;
        
        let task = IndexTask::history("test:123", Uuid::new_v4(), "hello");
        service.submit(task).await.unwrap();
        
        assert_eq!(service.queue_depth(), 1);
        
        service.graceful_shutdown(std::time::Duration::from_secs(1)).await;
    }
    
    #[tokio::test]
    async fn test_shutdown_rejects_new_tasks() {
        let service = create_test_service().await;
        service.shutdown.store(true, Ordering::Relaxed);
        
        let task = IndexTask::history("test:123", Uuid::new_v4(), "hello");
        service.submit(task).await.unwrap(); // Should not error, but drop silently
        
        assert_eq!(service.queue_depth(), 0); // Task was rejected
    }
}
```

- [ ] **Step 2: 更新 mod.rs**

修改 `engine/src/indexing/mod.rs`：

```rust
pub mod queue;
pub mod task;
pub mod service;

pub use queue::{PriorityQueue, Priority, QueueError};
pub use task::IndexTask;
pub use service::{IndexingService, IndexError};
```

- [ ] **Step 3: 运行测试**

```bash
cargo test --package gasket-engine indexing::service::tests --features local-embedding
```

Expected: Tests pass (可能需要安装 ONNX 库)

- [ ] **Step 4: Commit**

```bash
git add engine/src/indexing/
git commit -m "feat(indexing): add IndexingService skeleton

- Lazy TextEmbedder initialization
- Non-blocking task submission
- Graceful shutdown with timeout
- Queue depth monitoring"
```

---

### Task 5: 实现后台 Worker

**Files:**
- Create: `engine/src/indexing/worker.rs`
- Modify: `engine/src/indexing/service.rs`
- Modify: `engine/src/indexing/mod.rs`

**设计:**
实现后台 worker，从队列中取出任务并处理，包括 embedding 生成和数据库写入。

- [ ] **Step 1: 编写 worker.rs**

创建 `engine/src/indexing/worker.rs`：

```rust
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use sqlx::SqlitePool;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};

use crate::indexing::{PriorityQueue, IndexTask, Priority, IndexError};

#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;

pub struct IndexingWorker {
    queue: Arc<PriorityQueue<IndexTask>>,
    pool: SqlitePool,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    shutdown: Arc<AtomicBool>,
}

impl IndexingWorker {
    pub fn new(
        queue: Arc<PriorityQueue<IndexTask>>,
        pool: SqlitePool,
        #[cfg(feature = "local-embedding")]
        embedder: Option<Arc<TextEmbedder>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            queue,
            pool,
            #[cfg(feature = "local-embedding")]
            embedder,
            shutdown,
        }
    }
    
    pub async fn run(self) {
        info!("IndexingWorker started");
        
        while !self.shutdown.load(Ordering::Relaxed) {
            // Try to get a task with timeout to periodically check shutdown
            let task = match timeout(Duration::from_secs(1), self.queue.pop()).await {
                Ok(Some(task)) => task,
                Ok(None) => {
                    // Queue closed
                    break;
                }
                Err(_) => {
                    // Timeout, check shutdown flag
                    continue;
                }
            };
            
            if let Err(e) = self.process_task(&task).await {
                error!("Failed to process indexing task: {:?}", e);
                self.handle_failure(task, e).await;
            }
        }
        
        info!("IndexingWorker stopped");
    }
    
    async fn process_task(&self, task: &IndexTask) -> Result<(), IndexError> {
        match task {
            IndexTask::History { event_id, content, .. } => {
                self.index_history(*event_id, content).await?;
            }
            IndexTask::Memory { path, content, .. } => {
                self.index_memory(path, content).await?;
            }
            IndexTask::Batch { tasks, .. } => {
                for task in tasks {
                    if let Err(e) = self.process_task(task).await {
                        warn!("Batch task failed: {:?}", e);
                        // Continue processing other tasks in batch
                    }
                }
            }
        }
        Ok(())
    }
    
    async fn index_history(&self, event_id: Uuid, content: &str) -> Result<(), IndexError> {
        #[cfg(feature = "local-embedding")]
        {
            let embedder = self.embedder.as_ref()
                .ok_or(IndexError::NoEmbedder)?;
            
            // Check if already indexed
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM session_embeddings WHERE message_id = ?)"
            )
            .bind(event_id.to_string())
            .fetch_one(&self.pool)
            .await?;
            
            if exists {
                debug!("History event {} already indexed, skipping", event_id);
                return Ok(());
            }
            
            // Generate embedding
            let embedding = embedder.embed(content).await
                .map_err(|e| IndexError::Embedding(e.to_string()))?;
            
            // Save to database
            let embedding_bytes = bytemuck::cast_slice(&embedding);
            sqlx::query(
                "INSERT INTO session_embeddings (message_id, session_key, embedding, created_at) 
                 VALUES (?, '', ?, datetime('now'))"
            )
            .bind(event_id.to_string())
            .bind(embedding_bytes)
            .execute(&self.pool)
            .await?;
            
            debug!("Indexed history event {}", event_id);
        }
        
        #[cfg(not(feature = "local-embedding"))]
        {
            warn!("local-embedding feature not enabled, skipping indexing");
        }
        
        Ok(())
    }
    
    async fn index_memory(&self, path: &std::path::Path, content: &str) -> Result<(), IndexError> {
        // Extract body without frontmatter
        let body = extract_body(content);
        
        if body.len() < 10 {
            debug!("Skipping short memory body: {}", path.display());
            return Ok(());
        }
        
        #[cfg(feature = "local-embedding")]
        {
            let embedder = self.embedder.as_ref()
                .ok_or(IndexError::NoEmbedder)?;
            
            let path_str = path.to_string_lossy();
            
            // Check if already indexed and up to date
            // TODO: Add mtime check
            
            let embedding = embedder.embed(&body).await
                .map_err(|e| IndexError::Embedding(e.to_string()))?;
            
            let embedding_bytes = bytemuck::cast_slice(&embedding);
            sqlx::query(
                "INSERT OR REPLACE INTO memory_embeddings 
                 (memory_path, scenario, embedding, token_count, created_at, updated_at) 
                 VALUES (?, 'general', ?, ?, datetime('now'), datetime('now'))"
            )
            .bind(path_str.as_ref())
            .bind(embedding_bytes)
            .bind(body.len())
            .execute(&self.pool)
            .await?;
            
            debug!("Indexed memory file {}", path.display());
        }
        
        Ok(())
    }
    
    async fn handle_failure(&self, task: IndexTask, error: IndexError) {
        if task.retry_count() < 3 {
            let retry_task = task.with_incremented_retry()
                .with_priority(Priority::P2); // Downgrade to P2 for retry
            
            if let Err(e) = self.queue.push(retry_task, Priority::P2).await {
                error!("Failed to requeue task: {:?}", e);
            }
        } else {
            error!("Task failed after 3 retries: {:?}", error);
        }
    }
}

/// Extract body content without YAML frontmatter
fn extract_body(content: &str) -> &str {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return &content[end + 6..].trim_start();
        }
    }
    content
}

// Helper trait for setting priority
trait WithPriority {
    fn with_priority(self, priority: Priority) -> Self;
}

impl WithPriority for IndexTask {
    fn with_priority(self, priority: Priority) -> Self {
        match self {
            IndexTask::History { session_key, event_id, content, retry_count, .. } => {
                IndexTask::History { session_key, event_id, content, priority, retry_count }
            }
            IndexTask::Memory { path, content, retry_count, .. } => {
                IndexTask::Memory { path, content, priority, retry_count }
            }
            IndexTask::Batch { tasks, .. } => {
                IndexTask::Batch { tasks, priority }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extract_body() {
        let with_frontmatter = "---\ntitle: Test\n---\n\nHello world";
        assert_eq!(extract_body(with_frontmatter), "Hello world");
        
        let without_frontmatter = "Hello world";
        assert_eq!(extract_body(without_frontmatter), "Hello world");
    }
}
```

- [ ] **Step 2: 更新 service.rs 启动 worker**

修改 `engine/src/indexing/service.rs`：

```rust
use crate::indexing::worker::IndexingWorker;

// 在 start_background_worker 方法中:
pub fn start_background_worker(&self) {
    let worker = IndexingWorker::new(
        self.queue.clone(),
        self.pool.clone(),
        #[cfg(feature = "local-embedding")]
        self.embedder.clone(),
        self.shutdown.clone(),
    );
    
    tokio::spawn(async move {
        worker.run().await;
    });
    
    info!("Background worker started");
}
```

- [ ] **Step 3: 更新 mod.rs**

```rust
pub mod queue;
pub mod task;
pub mod service;
pub mod worker;

pub use queue::{PriorityQueue, Priority, QueueError};
pub use task::IndexTask;
pub use service::{IndexingService, IndexError};
```

- [ ] **Step 4: Commit**

```bash
git add engine/src/indexing/
git commit -m "feat(indexing): implement background worker

- Process tasks from priority queue
- Generate embeddings with fastembed
- Duplicate detection for history events
- Retry with downgrade to P2"
```

---

## Phase 3: 与现有系统集成

### Task 6: AgentLoop 集成

**Files:**
- Modify: `engine/src/agent/loop_.rs`
- Modify: `engine/src/agent/context.rs`

- [ ] **Step 1: 修改 AgentLoop 结构体**

在 `engine/src/agent/loop_.rs` 的 `AgentLoop` struct 中添加：

```rust
pub struct AgentLoop {
    // ... existing fields ...
    /// Indexing service for automatic embedding generation
    indexing_service: Option<Arc<IndexingService>>,
}
```

- [ ] **Step 2: 修改 with_services 构造函数**

```rust
async fn with_services(
    // ... existing params ...
) -> Result<Self, AgentError> {
    // ... existing store creation ...
    
    // Create IndexingService
    let indexing_service = Arc::new(
        IndexingService::new(sqlite_store.pool().clone()).await
            .map_err(|e| AgentError::Other(format!("Failed to create IndexingService: {}", e)))?
    );
    indexing_service.start_background_worker();
    
    // ... rest of initialization ...
    
    Ok(Self {
        // ... other fields ...
        indexing_service: Some(indexing_service),
    })
}
```

- [ ] **Step 3: 添加 save_and_index 辅助方法**

```rust
impl AgentLoop {
    /// Save event and trigger async indexing
    async fn save_and_index(&self, event: SessionEvent) -> Result<(), AgentError> {
        // 1. Save to database (blocking, ensures data safety)
        self.context.save_event(&event).await?;
        
        // 2. Submit indexing task (non-blocking)
        if should_index(&event) {
            if let Some(ref svc) = self.indexing_service {
                let task = IndexTask::history(
                    &event.session_key,
                    event.id,
                    &event.content,
                );
                // Fire and forget - don't block on indexing
                let _ = svc.submit(task).await;
            }
        }
        
        Ok(())
    }
}

fn should_index(event: &SessionEvent) -> bool {
    use gasket_types::EventType;
    
    match event.event_type {
        EventType::UserMessage | EventType::AssistantMessage => {
            event.content.len() >= 10
        }
        _ => false,
    }
}
```

- [ ] **Step 4: 更新 prepare_pipeline 使用 save_and_index**

在 `prepare_pipeline` 方法中，替换：
```rust
// 旧代码:
self.context.save_event(user_event).await?;

// 新代码:
self.save_and_index(user_event).await?;
```

- [ ] **Step 5: Commit**

```bash
git add engine/src/agent/
git commit -m "feat(agent): integrate IndexingService with AgentLoop

- Non-blocking indexing task submission
- Automatic indexing for UserMessage and AssistantMessage
- Content length filter (>= 10 chars)"
```

---

### Task 7: MemoryStore 集成

**Files:**
- Modify: `engine/src/agent/memory.rs`

- [ ] **Step 1: 添加 IndexingService 到 MemoryStore**

```rust
pub struct MemoryStore {
    base_dir: PathBuf,
    sqlite_store: Arc<SqliteStore>,
    indexing_service: Option<Arc<IndexingService>>,
}
```

- [ ] **Step 2: 修改 new 构造函数**

```rust
pub async fn new(
    base_dir: PathBuf,
    pool: &SqlitePool,
) -> anyhow::Result<Self> {
    // ... existing code ...
    
    Ok(Self {
        base_dir,
        sqlite_store: Arc::new(SqliteStore::from_pool(pool.clone())),
        indexing_service: None, // Will be set later
    })
}

pub fn with_indexing_service(mut self, service: Arc<IndexingService>) -> Self {
    self.indexing_service = Some(service);
    self
}
```

- [ ] **Step 3: 在 write_memory 中触发 indexing**

```rust
pub async fn write_memory(&self, path: &Path, content: &str) -> anyhow::Result<()> {
    // 1. Parse frontmatter
    let meta = parse_frontmatter(content)?;
    
    // 2. Check if indexing is enabled
    if meta.index != Some(false) {
        // 3. Submit indexing task
        if let Some(ref svc) = self.indexing_service {
            let task = IndexTask::memory(path.to_path_buf(), content);
            let _ = svc.submit(task).await;
        }
    }
    
    // ... rest of write logic ...
}
```

- [ ] **Step 4: Commit**

```bash
git add engine/src/agent/memory.rs
git commit -m "feat(memory): trigger indexing on memory write

- Check frontmatter index flag
- Non-blocking submission to IndexingService
- Optional integration (graceful if not set)"
```

---

## Phase 4: 测试

### Task 8: 集成测试

**Files:**
- Create: `engine/tests/indexing_integration.rs`

- [ ] **Step 1: 创建集成测试文件**

```rust
use gasket_engine::indexing::{IndexingService, IndexTask, Priority};
use sqlx::SqlitePool;
use uuid::Uuid;

async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    
    // Create required tables
    sqlx::query(
        "CREATE TABLE session_embeddings (
            message_id TEXT PRIMARY KEY,
            session_key TEXT NOT NULL,
            embedding BLOB NOT NULL,
            created_at TEXT NOT NULL
        )"
    )
    .execute(&pool)
    .await
    .unwrap();
    
    pool
}

#[tokio::test]
async fn test_indexing_service_lifecycle() {
    let pool = setup_test_db().await;
    let service = IndexingService::new(pool).await.unwrap();
    
    // Start worker
    service.start_background_worker();
    
    // Submit task
    let task = IndexTask::history("test:123", Uuid::new_v4(), "hello world test content");
    service.submit(task).await.unwrap();
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Shutdown
    service.graceful_shutdown(tokio::time::Duration::from_secs(5)).await;
}

#[tokio::test]
async fn test_duplicate_indexing_prevention() {
    let pool = setup_test_db().await;
    let service = IndexingService::new(pool.clone()).await.unwrap();
    
    service.start_background_worker();
    
    let event_id = Uuid::new_v4();
    
    // Submit same event twice
    let task1 = IndexTask::history("test:123", event_id, "content 1");
    let task2 = IndexTask::history("test:123", event_id, "content 2");
    
    service.submit(task1).await.unwrap();
    service.submit(task2).await.unwrap();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Verify only one entry in database
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_embeddings WHERE message_id = ?"
    )
    .bind(event_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    
    assert_eq!(count, 1);
}
```

- [ ] **Step 2: 运行集成测试**

```bash
cargo test --package gasket-engine --test indexing_integration --features local-embedding
```

- [ ] **Step 3: Commit**

```bash
git add engine/tests/
git commit -m "test(indexing): add integration tests

- Service lifecycle test
- Duplicate prevention test
- Uses in-memory SQLite"
```

---

## Phase 5: 导出和文档

### Task 9: 导出模块

**Files:**
- Modify: `engine/src/lib.rs`

- [ ] **Step 1: 添加 indexing 模块导出**

在 `engine/src/lib.rs` 中添加：

```rust
// Indexing module
#[cfg(feature = "local-embedding")]
pub mod indexing {
    pub use crate::indexing::*;
}

#[cfg(feature = "local-embedding")]
pub use indexing::{IndexingService, IndexTask, Priority, IndexError};
```

- [ ] **Step 2: Commit**

```bash
git add engine/src/lib.rs
git commit -m "feat(engine): export indexing module

- Available with local-embedding feature
- Public exports for IndexingService and types"
```

---

## 验证清单

- [ ] 所有单元测试通过
- [ ] 集成测试通过
- [ ] cargo clippy 无警告
- [ ] cargo fmt 格式化
- [ ] 文档注释完整

## 运行全部测试

```bash
# Unit tests
cargo test --package gasket-engine --features local-embedding

# Integration tests
cargo test --package gasket-engine --test indexing_integration --features local-embedding

# Check formatting
cargo fmt --check

# Clippy
cargo clippy --package gasket-engine --features local-embedding
```
