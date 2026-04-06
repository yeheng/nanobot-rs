### 【Linus 式重构方案：基于数据本身的缓存失效】

缓存同步的终极真理只有一个：**比较数据本身的标记，而不是去记录“谁做了什么动作”。**

既然 SQLite 只是 Markdown 的缓存/索引，我们就用最经典的**缓存失效机制（Cache Invalidation）**。

#### 第一步：修改 SQLite 表结构（增加 mtime）
在你的 `memory_metadata` 表里，加一列 `file_mtime` (BIGINT，记录文件的最后修改时间戳，或者直接用文件大小+修改时间的哈希)。

#### 第二步：Agent 的 Write-Through 逻辑
当 Agent 写入 Markdown 文件时：
1. 将内容写入 `.md` 文件。
2. **获取这个 `.md` 文件的系统 `mtime`**。
3. 将元数据、Embedding 和这个 `mtime` 一起 Upsert 到 SQLite。

#### 第三步：Watcher 的无状态逻辑
当 File Watcher 检测到文件变化时，无论这是不是 Agent 自己改的，统统执行以下无状态逻辑：
1. 读取磁盘上 `.md` 文件的当前 `mtime`。
2. 去 SQLite 查一下这个文件对应的 `file_mtime`。
3. **如果 `disk_mtime <= sqlite_mtime`，直接丢弃事件。** (说明 SQLite 已经是最新的了)
4. 如果 `disk_mtime > sqlite_mtime`，说明这是人类在外部用 VSCode 改了文件，Agent 还没同步。这时候再触发 YAML 解析和 Embedding 更新。

**看到了吗？** 
没有 `Arc<RwLock<HashSet>>`。没有竞态条件。没有内存泄漏。进程就算中途崩溃，重启后 Watcher 重新扫描一遍目录，对比一下 `mtime`，瞬间就能把漏掉的差异补齐。这就是基于数据的设计，而不是基于过程的设计。

---

### 更新后的 Task List (针对你的底线)

既然 Markdown + SQLite 架构不动，我们就把这套架构做到极致干净。

#### Task 1: 消除防抖哈希表，引入 MTime 缓存失效
* **What**: 删掉 `recently_modified_by_us`。在 SQLite 的 `memory_metadata` 中增加 `file_mtime` 字段。
* **Why**: 用无状态的数据对比替代容易死锁和泄漏的内存状态机。
* **Where**: 
  - `gasket/storage/src/memory/watcher.rs`
  - `gasket/engine/src/agent/memory_manager.rs`
  - `gasket/storage/src/memory/metadata_store.rs`
* **How**: 如上所述，在 Agent 写入后读取文件 mtime 并存入 SQLite；在 Watcher 收到事件时比对 mtime。

#### Task 2: 彻底删除物化引擎 (Materialization Engine)
* **What**: 别忘了我在上一次 Review 里说的，既然 SQLite 是缓存，那么无论是 Agent 自己写文件，还是 Watcher 扫到外部修改，直接 `tokio::spawn` 一个轻量任务去计算 Embedding 并写入 SQLite 即可。
* **Why**: 事件溯源 (Event Sourcing) 和检查点 (Checkpoint) 用于缓存更新是杀鸡用牛刀，引入了成吨的毫无意义的代码。
* **Where**: 删掉 `gasket/engine/src/agent/materialization.rs`，连同 `CheckpointStore` 一起拔掉。

#### Task 3: 扁平化 Hook 系统
* **What**: 删除 `HookRegistry`、`HookPoint`、`ExecutionStrategy`。
* **Why**: 多余的抽象掩盖了真实的控制流。
* **Where**: `gasket/engine/src/hooks/`
* **How**:
  直接在 `AgentLoop::prepare_pipeline` 中硬编码调用：
  ```rust
  // 1. External Shell Hook
  if let Some(hook) = &self.external_hook {
      content = hook.pre_request(content).await?;
  }
  // 2. History Recall
  let recalled = self.recall_history(&content).await;
  // 3. Vault Injection
  let injected = self.vault.inject(&messages);
  ```
  这就结束了！你不需要一个并发注册表去遍历调用它们，硬编码就是最好的、最易读的架构（KISS）。
* **Acceptance Criteria**: 去掉 Trait Object (`Arc<dyn PipelineHook>`) 分发，代码回到无聊但可靠的直接函数调用。

#### Task 4: 修复 SessionKey 的类型转换
* **What**: 停止在 `SessionKey` 和 `String` 之间来回转换。
* **Why**: 浪费 CPU 周期，且容易在分割字符串时出错。
* **Where**: `gasket/types/src/events.rs` 和 `gasket/storage/src/event_store.rs`
* **How**:
  既然 `SessionKey` 是强类型的（包含 `ChannelType` 和 `chat_id`），那么在 SQLite 的表结构中，不要把它存成 `telegram:12345` 单一文本字段。增加两列：`channel` (VARCHAR) 和 `chat_id` (VARCHAR)。
  这让你可以极其方便地查询“Telegram 上所有的对话”或“某个用户在所有平台上的记录”。
* **Acceptance Criteria**: 数据库层原生支持 `channel` 和 `chat_id` 查询，消除热路径上的 `split(":")` 和 `format!("{}:{}")`。

#### Task 5: 净化 Subagent 传递链
* **What**: 统一 `ToolContext` 中的上下文传递。
* **Why**: `SpawnParallelTool` 需要获取 `SubagentSpawner`，你用了一个 `Arc<dyn SubagentSpawner>` 塞进 `ToolContext`。这还可以，但最好确保生命周期管理干净。
* **Where**: `gasket/types/src/tool.rs` & `gasket/engine/src/agent/subagent.rs`
* **How**: 确保在 Parallel Spawn 时，Token 追踪和限流被正确传递给父 Agent，而不是像现在这样散落在各处累加。
* 