


基于现有信息，我理解您的需求是：**彻底抛弃 `rusqlite` 这个同步包袱，拒绝使用 `spawn_blocking` 的补丁方案，将整个项目的 SQLite 底层驱动切换为原生异步的 `sqlx`，从而从根本上解决 Tokio 运行时的阻塞隐患。**

请确认我的理解是否准确？

---

### 【Linus 的思考过程】

谢天谢地，你终于清醒了。

1. **"这是个真问题还是臆想出来的？"** - 绝对的真问题。把同步的 C 库（`libsqlite3`）用线程池强行包装塞进异步的事件循环里，本来就是脱裤子放屁。一旦并发量上来，你的线程池会被耗尽，整个 Agent 会陷入死锁。改为 `sqlx` 是釜底抽薪的正确决定。
2. **"有更简单的方法吗？"** - `sqlx` 内置了 `SqlitePool`。我们再也不需要手动去维护那个愚蠢的 `Arc<Mutex<Connection>>`，不需要去手动管理并发锁。把连接池的大小设好，剩下的交给底层去调度。
3. **"会破坏什么吗？"** - **不会破坏用户空间**。数据库文件格式（SQLite）是完全兼容的，表结构不需要改一行。`MemoryStore` 和 `SqliteTaskStore` 暴露给上层的 API 已经是 `async` 的了，所以对外接口也是零侵入。

### 【核心判断】
**绝对值得做：** 这是一个具备优秀工程品味的决定。既然你的整个网络层（Axum）、任务调度层（Tokio）、通道层（mpsc）都是完全异步的，那么最底层的磁盘 I/O 也必须是纯正的异步实现。不要在异步的宇宙里留下一个同步的黑洞。

### 【关键洞察】
- **数据结构**：你原来最大的败笔是 `Arc<Mutex<rusqlite::Connection>>`。换成 `sqlx::SqlitePool` 后，连接池的获取是无锁的（或者说锁争用极小），这是数据结构生命周期的本质飞跃。
- **复杂度**：动态 SQL 的拼装会有变化。`rusqlite` 里你手动拼装 `WHERE` 子句和参数数组，而在 `sqlx` 中你需要使用 `QueryBuilder`，这会让代码变得**更安全且更符合规范**。
- **风险点**：`sqlx` 在编译期检查 SQL 时可能无法处理动态构建的搜索语句（FTS5 search）。我们需要对这些特定查询放弃编译期宏（`query!`），改用运行时的 `sqlx::query` 或 `QueryBuilder`。

---

### 🛠 Task List (sqlx 重构计划)

为了把这个烂摊子收拾干净，你需要执行以下 4 个任务：

#### Task 1: 替换底层依赖与核心数据结构初始化
*   **What**: 移除 `rusqlite`，引入 `sqlx`（开启 `sqlite`, `runtime-tokio-rustls` 特性）。将核心的存储结构中的 `conn` 替换为 `SqlitePool`。
*   **Why**: 确立原生异步连接池，彻底干掉 `Mutex` 锁死运行时的隐患。
*   **Where**: 
    - `nanobot-core/Cargo.toml`
    - `nanobot-core/src/memory/sqlite.rs` (类 `SqliteStore`)
    - `nanobot-core/src/agent/task_store_sqlite.rs` (类 `SqliteTaskStore`)
*   **How**:
    1. 在 `Cargo.toml` 删除 `rusqlite`，添加 `sqlx = { version = "0.7", features =["sqlite", "runtime-tokio-rustls", "chrono", "uuid"] }`。
    2. 将 `SqliteStore` 中的 `conn: Arc<Mutex<Connection>>` 修改为 `pool: sqlx::SqlitePool`。
    3. 初始化代码改为使用 `sqlx::sqlite::SqlitePoolOptions::new().max_connections(5).connect_with(...)`。
*   **Test Case & Acceptance Criteria**:
    - `cargo check` 能够识别新的依赖并暴露所有之前返回 `rusqlite::Error` 的编译错误。
    - 数据库初始化时不阻塞主线程，文件正常创建或打开。

#### Task 2: 重写 Memory & Session CRUD 操作
*   **What**: 将 `memory/sqlite.rs` 中所有的同步执行语句（`execute`, `query_row`, `query_map`）翻译为 `sqlx` 的 `execute`, `fetch_one`, `fetch_optional`, `fetch_all`。
*   **Why**: 废除老的同步 SQL 语句，使读写操作真正让出协程控制权（yield）。
*   **Where**: `nanobot-core/src/memory/sqlite.rs`
*   **How**:
    1. 简单操作直接使用 `sqlx::query!(...).execute(&self.pool).await`。
    2. 对于 FTS5 全文搜索（`search_impl`），由于包含动态生成的 `LIMIT`, `OFFSET`, `tags` 过滤条件，使用 `sqlx::QueryBuilder` 来动态拼接 SQL 字符串和参数。
    ```rust
    // 伪代码示例：
    let mut qb = sqlx::QueryBuilder::new("SELECT m.* FROM memories m ");
    if let Some(text) = &query.text {
        qb.push("JOIN memories_fts f ON m.id = f.id WHERE memories_fts MATCH ");
        qb.push_bind(text);
    }
    let entries = qb.build_query_as::<MemoryEntry>().fetch_all(&self.pool).await?;
    ```
*   **Test Case & Acceptance Criteria**:
    - `cargo test test_sqlite_session_meta_and_messages` 和 `test_sqlite_fts5_search` 等原本的单元测试通过。
    - 验证新旧数据类型（特别是 Chrono 时间格式和 JSON 字符串）的编解码没有精度丢失。

#### Task 3: 重写 Subagent Task Store (任务持久化层)
*   **What**: 将 `task_store_sqlite.rs` 切换为 `sqlx` 实现，同时兼容现有的 Enum 整型映射（`status_to_int` 等）。
*   **Why**: Subagent 会产生高频的心跳和状态更新，必须保证无阻塞写入。
*   **Where**: `nanobot-core/src/agent/task_store_sqlite.rs`
*   **How**:
    1. `save_task` 使用 `sqlx::query!( "INSERT OR REPLACE INTO tasks (...) VALUES (?, ?, ...)", ...).execute(&self.pool).await`。
    2. 依然保留对 `tasks.json` 的一次性同步读取迁移（因为只发生在初始化阶段），但将迁移时的插入操作改为使用 `sqlx` 事务 (`self.pool.begin().await`)。
    3. 移除 `rusqlite::Row` 的相关手动解析，直接使用 `sqlx::FromRow` derive 或者 `fetch_all` 映射。
*   **Test Case & Acceptance Criteria**:
    - `cargo test test_sqlite_store_save_and_load` 通过。
    - 模拟 10 个子 Agent 同时变更任务状态，验证没有死锁且写入耗时低于 10ms。

#### Task 4: 平滑迁移旧版数据库初始化逻辑 (KISS)
*   **What**: 原来的 `conn.execute_batch` 用于建表，需要替换为 `sqlx` 的建表逻辑，但不要为了用工具而用工具（不要为了建表特地引入复杂的 sqlx-cli / migrations 文件夹体系）。
*   **Why**: 保持系统的轻量化，贯彻实用主义。你的 Agent 目前不需要企业级的版本回滚机制，只需要确保表存在即可。
*   **Where**: `nanobot-core/src/memory/sqlite.rs` (init_db 函数)
*   **How**:
    1. 在 `SqliteStore::new` 中，连接后直接执行一个封装了多个 `CREATE TABLE IF NOT EXISTS` 的大字符串。
    ```rust
    sqlx::query("
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS sessions (...);
        -- ...
    ").execute(&pool).await?;
    ```
*   **Test Case & Acceptance Criteria**:
    - 启动编译后的 `nanobot-cli` 并在一个干净的目录下运行，确保 `.nanobot/memory.db` 和相关表结构被正确创建。
    - 检查旧版本的 SQLite 数据库文件可以直接被打开读取，不需要破坏性重建。

去干活吧。写异步代码就得有异步的脑子，把那些 `Mutex<Connection>` 丢进垃圾桶里。完成后，你会拥有一套跑起来丝滑得像抹了油一样的核心存储层。
