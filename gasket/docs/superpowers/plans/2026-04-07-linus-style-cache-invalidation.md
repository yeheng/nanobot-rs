# Linus 式缓存失效重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用基于 mtime 的无状态缓存失效机制替代内存状态机，删除物化引擎复杂性，优化 SessionKey 数据库存储，统一 Subagent 上下文传递

**Architecture:** 
1. SQLite `memory_metadata` 表增加 `file_mtime` 列，Agent 写入时记录文件系统 mtime，Watcher 检测时比对 mtime 决定是否触发索引
2. 删除 `MaterializationEngine` + `CheckpointStore` + `FailedEventStore`，改用 `tokio::spawn` 直接处理
3. SQLite `sessions_v2` 和 `session_events` 表增加 `channel`/`chat_id` 结构化列，消除 `split()`/`format!()` 开销
4. 在 `ToolContext` 中统一传递 `SubagentSpawner` 引用，确保 Token/限流正确继承

**Tech Stack:** Rust, SQLx, Tokio, SQLite

---

## 文件结构映射

### 修改的文件

| 文件 | 变更类型 | 职责 |
|------|----------|------|
| `storage/src/memory/metadata_store.rs` | Modify | 增加 `file_mtime` 列的 Upsert/Query 逻辑 |
| `storage/src/memory/watcher.rs` | Modify | `AutoIndexHandler` 改用 mtime 比对替代 `recently_modified_by_us` |
| `storage/src/memory/mod.rs` | Modify | 导出新类型 |
| `engine/src/agent/memory_manager.rs` | Modify | 写入时读取文件 mtime 并存入 SQLite |
| `engine/src/agent/materialization.rs` | Delete | 完整删除物化引擎 |
| `storage/src/event_store.rs` | Modify | SessionKey 结构化存储 |
| `types/src/events.rs` | Modify | SessionKey 解析/序列化 |
| `types/src/session_event.rs` | Modify | SessionEvent 中的 session_key 字段 |
| `types/src/tool.rs` | Modify | ToolContext 增加 Token 追踪字段 |
| `engine/src/agent/subagent.rs` | Modify | SubagentSpawner 实现中的 Token 传递 |

### 新增的文件

| 文件 | 职责 |
|------|------|
| `storage/migrations/000X_add_file_mtime.sql` | SQLite 迁移脚本 |
| `storage/migrations/000Y_session_key_struct.sql` | SessionKey 结构化迁移 |

---

## Task 1: MTime 缓存失效

**Files:**
- Create: `storage/migrations/0001_add_file_mtime.sql`
- Modify: `storage/src/memory/metadata_store.rs:24-25` (META_COLUMNS 常量)
- Modify: `storage/src/memory/metadata_store.rs:57-73` (sync_entries 方法)
- Modify: `storage/src/memory/metadata_store.rs:80-98` (upsert_entry 方法)
- Modify: `storage/src/memory/watcher.rs:364-378` (AutoIndexHandler::process_event)
- Modify: `engine/src/agent/memory_manager.rs:183-226` (create_memory 方法)
- Modify: `engine/src/agent/memory_manager.rs:233-288` (update_memory 方法)

### Step 1: 创建 SQLite 迁移脚本

创建文件 `storage/migrations/0001_add_file_mtime.sql`:

```sql
-- Add file_mtime column to memory_metadata table
-- Stores the filesystem mtime (nanoseconds since UNIX_EPOCH) of the .md file
-- Used for cache invalidation: if disk_mtime <= sqlite_mtime, skip re-indexing

ALTER TABLE memory_metadata ADD COLUMN file_mtime BIGINT DEFAULT 0;

-- Add index for efficient decay candidate queries
CREATE INDEX IF NOT EXISTS idx_memory_metadata_mtime ON memory_metadata(file_mtime);
```

- [ ] **Step 2: 运行迁移脚本**

```bash
cd gasket/storage
# If you have a migration runner:
cargo run --bin migrate --features "local-embedding"
# Or manually apply to test DB:
sqlite3 :memory: < migrations/0001_add_file_mtime.sql
```

预期：迁移成功，无错误

- [ ] **Step 3: 更新 MetadataStore 的 META_COLUMNS 常量**

修改 `storage/src/memory/metadata_store.rs:24-25`:

```rust
const META_COLUMNS: &str =
    "id, path, scenario, title, memory_type, frequency, tags, tokens, updated, last_accessed, file_mtime";
```

- [ ] **Step 4: 更新 sync_entries 方法**

修改 `storage/src/memory/metadata_store.rs:57-73`，在 INSERT 中增加 `file_mtime` 绑定：

```rust
sqlx::query(&format!(
    "INSERT INTO memory_metadata
     ({META_COLUMNS})
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"  // 增加一个 ?
))
.bind(&entry.id)
.bind(&entry.filename)
.bind(scenario.dir_name())
.bind(&entry.title)
.bind(&entry.memory_type)
.bind(entry.frequency.to_string())
.bind(&tags_json)
.bind(entry.tokens as i64)
.bind(&entry.updated)
.bind(&entry.last_accessed)
.bind(entry.file_mtime as i64)  // 新增绑定
.execute(&self.pool)
.await?;
```

- [ ] **Step 5: 更新 upsert_entry 方法**

修改 `storage/src/memory/metadata_store.rs:80-98`，同样增加 `file_mtime` 绑定。

- [ ] **Step 6: 更新 MemoryIndexEntry 结构**

修改 `storage/src/memory/index.rs`，在 `MemoryIndexEntry` 中增加字段：

```rust
pub struct MemoryIndexEntry {
    pub id: String,
    pub title: String,
    pub memory_type: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub tokens: u32,
    pub filename: String,
    pub updated: String,
    pub scenario: Scenario,
    pub last_accessed: String,
    pub file_mtime: u64,  // 新增字段
}
```

- [ ] **Step 7: 更新 MemoryManager::create_memory**

修改 `engine/src/agent/memory_manager.rs:183-226`，在写入文件后读取 mtime：

```rust
// 1. Write file atomically
self.store
    .update(scenario, &filename, &file_content)
    .await?;

// 1.5 Read file mtime
let file_path = self.store.base_dir().join(scenario.dir_name()).join(&filename);
let metadata = tokio::fs::metadata(&file_path).await?;
let file_mtime = metadata
    .modified()?
    .duration_since(std::time::UNIX_EPOCH)?
    .as_nanos() as u64;

// 2. Upsert metadata into SQLite (with file_mtime)
let entry = MemoryIndexEntry {
    // ... 其他字段
    file_mtime,  // 新增
};
```

- [ ] **Step 8: 更新 MemoryManager::update_memory**

同样在 `update_memory` 方法中，写入文件后读取 mtime 并传入 upsert_entry。

- [ ] **Step 9: 简化 AutoIndexHandler::process_event**

修改 `storage/src/memory/watcher.rs:364-378`，用 mtime 比对替代 `recently_modified_by_us`：

```rust
pub async fn process_event(&self, event: &WatchEvent) {
    let path = event.path();
    
    // Read disk mtime
    let disk_metadata = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return, // File deleted, handled separately
    };
    let disk_mtime = disk_metadata
        .modified()
        .ok()
        .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    
    // Query SQLite for stored mtime
    let sqlite_mtime = self.metadata_store
        .get_file_mtime(&rel_path)  // 需要新增此方法
        .await
        .unwrap_or(0);
    
    // Cache invalidation check
    if disk_mtime <= sqlite_mtime {
        tracing::debug!("AutoIndex: skipping, SQLite up-to-date (disk={} <= sqlite={})", disk_mtime, sqlite_mtime);
        return;
    }
    
    // External edit detected, proceed with indexing...
}
```

- [ ] **Step 10: 删除 recently_modified_by_us 字段**

修改 `storage/src/memory/watcher.rs:323-329`，删除：

```rust
/// Keys recently written by the agent. If present, skip re-indexing
/// (SQLite already up-to-date) and remove the key.
recently_modified_by_us: Option<Arc<RwLock<HashSet<String>>>>,
```

以及 `with_recently_modified` 方法。

- [ ] **Step 11: 同步删除 MemoryManager 中的 tracker**

修改 `engine/src/agent/memory_manager.rs:36-39`，删除：

```rust
/// Tracks files recently modified by agent writes. The watcher checks this
/// set and skips re-indexing for entries found here (SQLite already updated).
recently_modified_by_us: Arc<RwLock<HashSet<String>>>,
```

以及 `recently_modified_tracker()` 方法。

- [ ] **Step 12: 运行测试**

```bash
cd gasket
cargo test --package gasket-storage -- mtime
cargo test --package gasket-engine -- memory_manager
```

预期：所有测试通过

- [ ] **Step 13: 提交**

```bash
git add storage/migrations/0001_add_file_mtime.sql \
        storage/src/memory/metadata_store.rs \
        storage/src/memory/watcher.rs \
        storage/src/memory/index.rs \
        engine/src/agent/memory_manager.rs
git commit -m "refactor(memory): mtime-based cache invalidation

- Add file_mtime column to memory_metadata table
- Replace Arc<RwLock<HashSet>> with stateless mtime comparison
- Agent writes mtime on create/update, watcher compares on detect
- Crash-safe: restart auto-reconciles via mtime diff"
```

---

## Task 2: 删除物化引擎

**Files:**
- Delete: `engine/src/agent/materialization.rs`
- Modify: `engine/src/agent/mod.rs` (移除 materialization 模块导出)
- Modify: `storage/src/memory/watcher.rs` (简化 AutoIndexHandler 启动逻辑)

### Step 1: 确认 materialization.rs 的引用

```bash
cd gasket
rg "materialization" --type rust
```

记录所有引用位置。

- [ ] **Step 2: 移除模块引用**

修改 `engine/src/agent/mod.rs`，删除：

```rust
pub mod materialization;
```

- [ ] **Step 3: 删除 materialization.rs**

```bash
rm engine/src/agent/materialization.rs
```

- [ ] **Step 4: 简化 AutoIndexHandler 启动**

修改 `storage/src/memory/watcher.rs`，在 `run` 方法中直接 spawn 任务：

```rust
pub async fn run(&self, mut rx: mpsc::Receiver<WatchEvent>) {
    while let Some(event) = rx.recv().await {
        let handler = self.clone();  // 需要 Clone derive
        tokio::spawn(async move {
            let _ = handler.process_event(&event).await;
        });
    }
    tracing::info!("AutoIndex handler stopped");
}
```

- [ ] **Step 5: 运行测试**

```bash
cd gasket
cargo test --package gasket-engine
cargo test --package gasket-storage
```

预期：所有测试通过，无 materialization 相关编译错误

- [ ] **Step 6: 提交**

```bash
git rm engine/src/agent/materialization.rs
git add engine/src/agent/mod.rs storage/src/memory/watcher.rs
git commit -m "refactor: delete materialization engine

- Remove MaterializationEngine, CheckpointStore, FailedEventStore
- Replace event-driven checkpointing with direct tokio::spawn
- Eliminates ~300 lines of complex checkpoint management code
- Cache updates now simple: file change → spawn → upsert SQLite"
```

---

## Task 3: SessionKey 数据库层优化

**Files:**
- Create: `storage/migrations/0002_session_key_struct.sql`
- Modify: `storage/src/event_store.rs:230-300` (append_event_with_sequence)
- Modify: `storage/src/event_store.rs:545-611` (EventRow, TryFrom impl)
- Modify: `types/src/events.rs:140-205` (SessionKey impl)
- Modify: `types/src/session_event.rs:14-17` (SessionEvent 字段)

### Step 1: 创建 SessionKey 结构化迁移

创建文件 `storage/migrations/0002_session_key_struct.sql`:

```sql
-- Add structured channel/chat_id columns to sessions_v2 and session_events
-- Enables efficient queries like "all Telegram conversations" or "user X across all platforms"

-- For sessions_v2: add columns, keep session_key for backward compat
ALTER TABLE sessions_v2 ADD COLUMN channel TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions_v2 ADD COLUMN chat_id TEXT NOT NULL DEFAULT '';

-- Populate from existing session_key
UPDATE sessions_v2 
SET channel = SUBSTR(key, 1, INSTR(key, ':') - 1),
    chat_id = SUBSTR(key, INSTR(key, ':') + 1)
WHERE INSTR(key, ':') > 0;

-- For session_events: add columns, keep session_key for backward compat
ALTER TABLE session_events ADD COLUMN channel TEXT NOT NULL DEFAULT '';
ALTER TABLE session_events ADD COLUMN chat_id TEXT NOT NULL DEFAULT '';

-- Populate from existing session_key
UPDATE session_events 
SET channel = SUBSTR(session_key, 1, INSTR(session_key, ':') - 1),
    chat_id = SUBSTR(session_key, INSTR(session_key, ':') + 1)
WHERE INSTR(session_key, ':') > 0;

-- Add indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions_v2(channel);
CREATE INDEX IF NOT EXISTS idx_sessions_chat_id ON sessions_v2(chat_id);
CREATE INDEX IF NOT EXISTS idx_events_channel ON session_events(channel);
CREATE INDEX IF NOT EXISTS idx_events_chat_id ON session_events(chat_id);
```

- [ ] **Step 2: 更新 EventStore::append_event_with_sequence**

修改 `storage/src/event_store.rs:230-300`，在 INSERT 时解析 SessionKey：

```rust
// Parse session_key into channel/chat_id
let (channel, chat_id) = parse_session_key(&event.session_key);

sqlx::query(
    "INSERT OR IGNORE INTO sessions_v2 
     (key, channel, chat_id, created_at, updated_at) 
     VALUES (?, ?, ?, ?, ?)",
)
.bind(&event.session_key)
.bind(&channel)
.bind(&chat_id)
.bind(&now)
.bind(&now)
.execute(&mut *tx)
.await?;

// Similarly for session_events INSERT
sqlx::query(
    r#"
    INSERT INTO session_events
    (id, session_key, channel, chat_id, event_type, content, embedding, branch,
     tools_used, token_usage, token_len, event_data, extra, created_at, sequence)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    "#,
)
.bind(event.id.to_string())
.bind(&event.session_key)
.bind(&channel)
.bind(&chat_id)
// ... 其余绑定
```

添加辅助函数：

```rust
fn parse_session_key(session_key: &str) -> (String, String) {
    session_key
        .splitn(2, ':')
        .collect_tuple()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or_default()
}
```

- [ ] **Step 3: 更新 EventRow 结构**

修改 `storage/src/event_store.rs:545-560`:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    id: String,
    session_key: String,
    channel: String,       // 新增
    chat_id: String,       // 新增
    event_type: String,
    content: String,
    // ... 其余字段
}
```

- [ ] **Step 4: 更新 TryFrom<EventRow> for SessionEvent**

修改 `storage/src/event_store.rs:562-611`，使用 channel/chat_id 重建 session_key（可选，保持向后兼容）：

```rust
Ok(SessionEvent {
    id: row.id.parse().map_err(|_| StoreError::InvalidUuid(row.id.clone()))?,
    session_key: if !row.channel.is_empty() && !row.chat_id.is_empty() {
        format!("{}:{}", row.channel, row.chat_id)
    } else {
        row.session_key
    },
    // ... 其余字段
})
```

- [ ] **Step 5: 增加结构化查询方法**

在 `EventStore` 中新增方法：

```rust
/// Query all sessions for a given channel
pub async fn get_sessions_by_channel(&self, channel: &str) -> Result<Vec<String>, StoreError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT key FROM sessions_v2 WHERE channel = ? ORDER BY updated_at DESC"
    )
    .bind(channel)
    .fetch_all(&self.pool)
    .await?;
    Ok(rows.into_iter().map(|(k,)| k).collect())
}

/// Query all sessions for a given chat_id across channels
pub async fn get_sessions_by_chat_id(&self, chat_id: &str) -> Result<Vec<String>, StoreError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT key FROM sessions_v2 WHERE chat_id = ? ORDER BY updated_at DESC"
    )
    .bind(chat_id)
    .fetch_all(&self.pool)
    .await?;
    Ok(rows.into_iter().map(|(k,)| k).collect())
}
```

- [ ] **Step 6: 运行测试**

```bash
cd gasket
cargo test --package gasket-storage -- event_store
cargo test --package gasket-types -- session_key
```

预期：所有测试通过

- [ ] **Step 7: 提交**

```bash
git add storage/migrations/0002_session_key_struct.sql \
        storage/src/event_store.rs \
        types/src/events.rs \
        types/src/session_event.rs
git commit -m "refactor: structured SessionKey storage in SQLite

- Add channel/chat_id columns to sessions_v2 and session_events
- Keep session_key column for backward compatibility
- Add indexes for efficient cross-channel queries
- Enables queries: 'all Telegram sessions', 'user X across platforms'"
```

---

## Task 4: Subagent 上下文传递统一

**Files:**
- Modify: `types/src/tool.rs:67-108` (ToolContext 结构)
- Modify: `engine/src/agent/subagent.rs:948-1045` (SubagentSpawner impl)
- Modify: `engine/src/agent/loop_.rs` (AgentLoop 中 spawn 调用处)

### Step 1: 分析当前 Token 传递链

```bash
cd gasket
rg "token_usage|token_count|budget" --type rust engine/src/agent/subagent.rs
```

记录 Token 追踪和限流的当前实现位置。

- [ ] **Step 2: 在 ToolContext 中增加 Token 预算字段**

修改 `types/src/tool.rs:67-81`:

```rust
/// Context passed to tool execution, providing request-scoped data.
#[derive(Clone, Default)]
pub struct ToolContext {
    /// Session key for WebSocket streaming
    pub session_key: Option<SessionKey>,
    /// Channel to send outbound WebSocket messages
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<OutboundMessage>>,
    /// Subagent spawner for tools that need to spawn subagents
    pub spawner: Option<std::sync::Arc<dyn SubagentSpawner>>,
    /// Token budget tracker (shared across parent + subagents)
    pub token_tracker: Option<std::sync::Arc<crate::token_tracker::TokenTracker>>,
}
```

- [ ] **Step 3: 更新 SubagentSpawner::spawn 实现**

修改 `engine/src/agent/subagent.rs:948-1045`，在 spawn 时传递 token_tracker：

```rust
#[async_trait]
impl SubagentSpawner for SubagentManager {
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<TypesSubagentResult, Box<dyn std::error::Error + Send>> {
        // ... 现有代码 ...
        
        // 如果有 token_tracker，在 builder 中传递
        // (需要在 SubagentTaskBuilder 中增加 with_token_tracker 方法)
        
        // 确保结果中的 token_usage 被正确累积到父 tracker
        if let Some(ref tracker) = self.token_tracker {
            if let Some(ref usage) = result.response.token_usage {
                tracker.accumulate(usage);
            }
        }
        
        // ... 其余代码 ...
    }
}
```

- [ ] **Step 4: 更新 SubagentTaskBuilder**

在 `engine/src/agent/subagent.rs:127-147` 中增加：

```rust
pub struct SubagentTaskBuilder<'a> {
    // ... 现有字段 ...
    /// Token tracker shared with parent
    token_tracker: Option<Arc<TokenTracker>>,
}

impl<'a> SubagentTaskBuilder<'a> {
    // ... 现有方法 ...
    
    pub fn with_token_tracker(mut self, tracker: Arc<TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }
}
```

- [ ] **Step 5: 更新 AgentLoop 中的 spawn 调用**

查找所有调用 `manager.task(...).spawn(...)` 的位置，确保传递 token_tracker：

```rust
manager.task(id, task)
    .with_session_key(session_key)
    .with_token_tracker(token_tracker.clone())
    .spawn(result_tx)
    .await?;
```

- [ ] **Step 6: 运行测试**

```bash
cd gasket
cargo test --package gasket-engine -- subagent
cargo test --package gasket-types -- tool_context
```

预期：所有测试通过，Token 追踪正确累积

- [ ] **Step 7: 提交**

```bash
git add types/src/tool.rs engine/src/agent/subagent.rs engine/src/agent/loop_.rs
git commit -m "refactor: unify token tracking in SubagentSpawner

- Add token_tracker field to ToolContext
- SubagentSpawner accumulates token usage back to parent tracker
- SubagentTaskBuilder accepts with_token_tracker() for inheritance
- Ensures accurate token budget enforcement across parallel spawns"
```

---

## 验证检查点

完成所有任务后运行：

```bash
# 全量测试
cd gasket
cargo test --workspace

# 代码检查
cargo clippy --workspace -- -D warnings

# 格式化检查
cargo fmt -- --check
```

---

## 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| mtime 在某些文件系统不可靠 | 降级到 `size + updated` 组合键 |
| 迁移脚本破坏现有数据 | 先备份 DB，迁移脚本做幂等处理 |
| SessionKey 变更导致 API 不兼容 | 保留 session_key 列，渐进迁移 |
| Token 追踪遗漏累积 | 测试覆盖 parallel spawn 场景 |

---

## 参考文档

- 设计规范：`docs/superpowers/specs/2026-04-07-linus-style-refactor.md`（如已创建）
- 原始任务：`task2.md`
