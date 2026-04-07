# 自动 Indexing 功能实施计划 (修订版)

> **For agentic workers:** REQUIRED SUB-SKILL: Use @superpowers:subagent-driven-development (recommended) or @superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在现有 indexing 基础上添加优先级队列和记忆文件自动 indexing

**Architecture:** 扩展现有 `IndexingService` 添加异步队列，同时在 `storage/src/memory/` 中添加记忆文件写入触发

**Tech Stack:** Rust, tokio, sqlx, fastembed (local-embedding feature)

---

## 关键发现与调整

### 现有代码分析

1. **已有 `IndexingService`** (`engine/src/agent/indexing.rs`)
   - 同步批处理设计
   - 用于处理驱逐事件的 indexing
   - 由 `ContextCompactor` 调用

2. **已有自动 embedding** (`PersistentContext::save_event`)
   - 实时生成 embedding
   - 内联处理，无队列

3. **记忆文件写入** (`storage/src/memory/`)
   - 通过 `MemoryFile` 和 `MetadataStore` 操作
   - 需要在此处添加 indexing 触发

### 实施策略调整

**不替换现有系统，而是：**
1. 扩展现有 `IndexingService` 添加优先级队列
2. 添加新的异步 indexing API (`submit` / `process_queue`)
3. 在记忆文件写入路径添加 indexing 调用

---

## Phase 1: 添加 MemoryMeta.index 字段

### Task 1: 修改 MemoryMeta 结构体

**Files:**
- Modify: `storage/src/memory/types.rs`

- [ ] **Step 1: 添加 index 字段**

```rust
pub struct MemoryMeta {
    // ... existing fields ...
    
    /// Whether to index this memory for search (default: true)
    #[serde(default = "default_true")]
    pub index: bool,
}

fn default_true() -> bool { true }
```

- [ ] **Step 2: 更新 Default impl**

```rust
impl Default for MemoryMeta {
    fn default() -> Self {
        // ...
        index: true,
        // ...
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add storage/src/memory/types.rs
git commit -m "feat(storage): add index field to MemoryMeta"
```

---

## Phase 2: 扩展 IndexingService

### Task 2: 创建 IndexingQueue 模块

**Files:**
- Create: `engine/src/agent/indexing_queue.rs`

- [ ] **Step 1: 创建优先级队列**

```rust
use tokio::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    P0 = 0,
    P1 = 1,
    P2 = 2,
}

pub struct IndexingQueue<T> {
    p0_tx: mpsc::Sender<T>,
    p0_rx: mpsc::Receiver<T>,
    p1_tx: mpsc::Sender<T>,
    p1_rx: mpsc::Receiver<T>,
    p2_tx: mpsc::Sender<T>,
    p2_rx: mpsc::Receiver<T>,
    depth: Arc<AtomicUsize>,
    max_depth: usize,
}

impl<T> IndexingQueue<T> {
    pub fn new(max_depth: usize) -> Self {
        let per_queue = max_depth / 3;
        let (p0_tx, p0_rx) = mpsc::channel(per_queue);
        let (p1_tx, p1_rx) = mpsc::channel(per_queue);
        let (p2_tx, p2_rx) = mpsc::channel(per_queue);
        
        Self {
            p0_tx, p0_rx, p1_tx, p1_rx, p2_tx, p2_rx,
            depth: Arc::new(AtomicUsize::new(0)),
            max_depth,
        }
    }
    
    pub async fn push(&self, item: T, priority: Priority) -> Result<(), QueueError> {
        let (tx, counter) = match priority {
            Priority::P0 => (&self.p0_tx, 0),
            Priority::P1 => (&self.p1_tx, 1),
            Priority::P2 => (&self.p2_tx, 2),
        };
        
        match tx.try_send(item) {
            Ok(_) => {
                self.depth.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => Err(QueueError::Full),
        }
    }
    
    pub async fn pop(&mut self) -> Option<T> {
        let result = tokio::select! {
            biased;
            item = self.p0_rx.recv() => item,
            item = self.p1_rx.recv() => item,
            item = self.p2_rx.recv() => item,
        };
        
        if result.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }
        
        result
    }
    
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Queue is full")]
    Full,
}
```

- [ ] **Step 2: Commit**

```bash
git add engine/src/agent/indexing_queue.rs
git commit -m "feat(indexing): add IndexingQueue with priority levels"
```

---

### Task 3: 扩展 IndexingService

**Files:**
- Modify: `engine/src/agent/indexing.rs`
- Modify: `engine/src/agent/mod.rs`

- [ ] **Step 1: 添加异步任务类型**

在 `indexing.rs` 中添加：

```rust
/// Async indexing task for queue-based processing
#[derive(Debug, Clone)]
pub enum AsyncIndexTask {
    /// Index a single history event
    History {
        session_key: String,
        event_id: Uuid,
        content: String,
    },
    /// Index a memory file
    Memory {
        path: std::path::PathBuf,
        content: String,
    },
}
```

- [ ] **Step 2: 扩展现有结构体**

```rust
pub struct IndexingService {
    store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    /// Async task queue (optional, set via enable_queue)
    queue: Option<Arc<IndexingQueue<AsyncIndexTask>>>,
    shutdown: Arc<AtomicBool>,
}
```

- [ ] **Step 3: 添加队列支持和后台处理**

```rust
impl IndexingService {
    /// Enable async queue processing
    pub fn enable_queue(&mut self, max_depth: usize) {
        let queue = Arc::new(IndexingQueue::new(max_depth));
        self.queue = Some(queue);
    }
    
    /// Submit async indexing task
    pub async fn submit(&self, task: AsyncIndexTask, priority: Priority) -> Result<(), QueueError> {
        let Some(ref queue) = self.queue else {
            // Queue not enabled, process synchronously
            self.process_task(&task).await;
            return Ok(());
        };
        
        if self.shutdown.load(Ordering::Relaxed) {
            return Ok(()); // Graceful degradation
        }
        
        queue.push(task, priority).await
    }
    
    /// Start background worker
    pub fn start_worker(&self) {
        let Some(queue) = self.queue.clone() else { return };
        let store = self.store.clone();
        let embedder = self.embedder.clone();
        let shutdown = self.shutdown.clone();
        
        tokio::spawn(async move {
            let mut worker = IndexingWorker {
                queue,
                store,
                embedder,
                shutdown,
            };
            worker.run().await;
        });
    }
    
    /// Process single task (sync)
    async fn process_task(&self, task: &AsyncIndexTask) {
        #[cfg(feature = "local-embedding")]
        {
            let Some(ref embedder) = self.embedder else { return };
            
            match task {
                AsyncIndexTask::History { session_key, event_id, content } => {
                    // Check if already indexed
                    if self.store.has_embedding(&event_id.to_string()).await.unwrap_or(false) {
                        return;
                    }
                    
                    if let Ok(embedding) = embedder.embed(content).await {
                        let _ = self.store.save_embedding(
                            &event_id.to_string(),
                            session_key,
                            &embedding
                        ).await;
                    }
                }
                AsyncIndexTask::Memory { path, content } => {
                    // Extract body without frontmatter
                    let body = extract_body(content);
                    if body.len() < 10 { return; }
                    
                    if let Ok(embedding) = embedder.embed(&body).await {
                        let path_str = path.to_string_lossy();
                        // Save to memory_embeddings table
                        let _ = sqlx::query(
                            "INSERT OR REPLACE INTO memory_embeddings 
                             (memory_path, scenario, embedding, token_count, updated_at) 
                             VALUES (?1, 'general', ?2, ?3, datetime('now'))"
                        )
                        .bind(path_str.as_ref())
                        .bind(bytemuck::cast_slice(&embedding))
                        .bind(body.len() as i64)
                        .execute(&self.store.pool())
                        .await;
                    }
                }
            }
        }
    }
}

fn extract_body(content: &str) -> &str {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return &content[end + 6..].trim_start();
        }
    }
    content
}

struct IndexingWorker {
    queue: Arc<IndexingQueue<AsyncIndexTask>>,
    store: Arc<SqliteStore>,
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
    shutdown: Arc<AtomicBool>,
}

impl IndexingWorker {
    async fn run(&mut self) {
        while !self.shutdown.load(Ordering::Relaxed) {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(1),
                self.queue.pop()
            ).await {
                Ok(Some(task)) => self.process_task(&task).await,
                _ => continue,
            }
        }
    }
    
    async fn process_task(&self, task: &AsyncIndexTask) {
        // Same as IndexingService::process_task
    }
}
```

- [ ] **Step 4: 更新 mod.rs 导出**

在 `engine/src/agent/mod.rs` 中添加：

```rust
pub mod indexing_queue;
pub use indexing_queue::{IndexingQueue, Priority, QueueError};
```

- [ ] **Step 5: Commit**

```bash
git add engine/src/agent/
git commit -m "feat(indexing): extend IndexingService with async queue support

- Add IndexingQueue for priority-based processing
- Add AsyncIndexTask for history and memory
- Backward compatible with existing sync API"
```

---

## Phase 3: 集成点

### Task 4: AgentLoop 启用队列

**Files:**
- Modify: `engine/src/agent/loop_.rs`

- [ ] **Step 1: 在 with_services 中启用队列**

找到 `IndexingService` 创建的地方，添加：

```rust
// After creating IndexingService
indexing_service.enable_queue(10000);
indexing_service.start_worker();
```

- [ ] **Step 2: Commit**

```bash
git add engine/src/agent/loop_.rs
git commit -m "feat(agent): enable async indexing queue in AgentLoop"
```

---

### Task 5: 记忆文件写入触发

**Files:**
- Modify: `storage/src/memory/store.rs` 或 `storage/src/memory/lifecycle.rs`

需要先找到记忆文件实际写入的位置：

```bash
grep -r "write_memory\|fs::write\|tokio::fs::write" storage/src/memory/ --include="*.rs"
```

- [ ] **Step 1: 定位记忆文件写入点**

- [ ] **Step 2: 添加 indexing 触发**

```rust
// After writing memory file
if meta.index != Some(false) {
    // Get reference to IndexingService and submit task
    // This may require passing IndexingService through the call chain
}
```

- [ ] **Step 3: Commit**

---

## Phase 4: 测试

### Task 6: 单元测试

- [ ] **测试 IndexingQueue 优先级**
- [ ] **测试 AsyncIndexTask 序列化**
- [ ] **测试 extract_body 函数**

### Task 7: 集成测试

- [ ] **测试端到端 indexing 流程**
- [ ] **测试队列积压处理**

---

## 验证命令

```bash
# Check compilation
cargo check --package gasket-engine --features local-embedding

# Run tests
cargo test --package gasket-engine --features local-embedding

# Check formatting
cargo fmt --check
```
