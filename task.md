这是一个非常关键的架构问题，它决定了你的 AI 助理是“看起来聪明”还是“真的聪明”。

在回答你之前，我们先用“好品味”的标准审视一下“上下文”和“记忆”的本质区别。

在计算机体系结构里：
- **历史记录 (History/Session)** = **L1/L2 Cache 或 RAM**。它是短期的、易失的、自动管理的。
- **长期记忆 (Memory/Markdown)** = **硬盘 (Disk)**。它是长期的、结构化的、需要明确指令去读写的。

基于这个哲学，我的核心判断是：**必须为长期记忆（Markdown）提供强大且专用的 LLM Tool，但应尽量剥夺或弱化 LLM 对短期历史（History）的直接工具访问权。**

我看了你现有的代码（`MemorySearchTool` 和 `HistorySearchTool`），写的非常糟糕。你在用上世纪 90 年代 Bash 脚本的思维给 2024 年的 LLM 写工具。

让我告诉你为什么，以及该怎么改。

---

### 【关键洞察与现状批判】

#### 1. 历史记录模块 (History) —— 为什么不需要（或只需要极简的）Tool？
你当前的 `HistorySearchTool` 允许 LLM 去 SQLite 里跑 `LIKE %query%` 查询。
**这是在制造精神分裂。**
按照我们上一轮的重构方案，旧的历史会被压缩成 Summary，而最旧的原始 Event 会被丢弃。如果 LLM 去查 `session_events` 表，它查到的将是碎片化的信息。
更重要的是：**你不应该指望 LLM 像 DBA 一样去查聊天记录。** 当前的 Summary 和 Recent Events 已经通过 System Prompt 自动注入到上下文里了（这就是缓存命中的概念）。如果一条信息重要到几周后还需要被想起来，它**根本就不该留在历史记录里，它早就该被写入长期记忆（Markdown）了！**

#### 2. 长期记忆模块 (Memory) —— 为什么现有的 Tool 是垃圾？
看看你的 `MemorySearchTool::search_with_filesystem`：
你竟然在每次搜索时，遍历 `~/.gasket/memory/` 下的**所有** `.md` 文件，把文件读进内存，然后用 `line.to_lowercase().contains(&query_lower)` 去做文本匹配？！
**你在干什么？！** 你辛辛苦苦在 `storage` 模块里写了 `EmbeddingStore`，集成了 `fastembed` 算向量，建了 SQLite 索引，结果 LLM 工具用的却是最原始的全文 grep？！这不仅慢得令人发指，而且毫无语义理解能力。

另外，你目前依靠通用的 `write_file` 工具让 LLM 去写记忆。LLM 懂个屁的 YAML Frontmatter！如果它忘了写 `---`，或者把 `tags` 格式写错了，你的解析器立马原地爆炸。

---

### 【Linus式方案 & Task List: LLM Tool 重构】

把这些愚蠢的 grep 操作和数据库直连操作扔掉。我们需要为 LLM 提供一组高层语义抽象的“大脑海马体”工具。

#### Task 1: 废弃并重写 `MemorySearchTool` (Use the Damn Vector DB)
* **What**: 将 `MemorySearchTool` 的底层实现从文件系统 `grep` 彻底替换为调用 `RetrievalEngine::search`（基于向量和标签的混合检索）。
* **Why**: LLM 需要的是语义搜索（“我喜欢吃什么”），而不是字面量匹配。你已经有了计算 Cosine Similarity 的代码，为什么不用？
* **Where**: `gasket/engine/src/tools/memory_search.rs`。
* **How**:
  1. 移除 `fs::read_dir` 和 `contains` 的垃圾代码。
  2. 让 Tool 接收 `query` (字符串) 和 `tags` (可选数组)。
  3. 直接将这些参数传递给 `RetrievalEngine`，返回 Top-K 个最相关的文件名和摘要（Frontmatter 里的 Title 和 Description，加上部分 Content）。
* **Test Case & Acceptance Criteria**: 搜索 "database design" 能够基于语义匹配到标题为 "PostgreSQL architecture" 的文件，即使文件中没有包含确切的 "database design" 字符串。

#### Task 2: 创建专用的 `MemorizeTool` (Protect the Frontmatter)
* **What**: 新增一个工具 `memorize`，专门用于让 LLM 写入或更新长期记忆，取代让 LLM 直接使用 `write_file` 写 `.md` 文件的行为。
* **Why**: "不要相信用户的输入，更不要相信 LLM 的输出。" 你不能指望 LLM 每次都能完美地构造出符合你规范的 YAML Frontmatter。必须用强类型接口把它隔离开。
* **Where**: 新建 `gasket/engine/src/tools/memorize.rs`。
* **How**:
  1. 定义 Tool JSON Schema: `title` (string), `content` (string), `scenario` (enum: profile, knowledge, etc.), `tags` (array of strings)。
  2. 在 Tool 的 Rust 实现中：接收这些标准参数，**由 Rust 代码负责生成和拼接 YAML Frontmatter**。
  3. 最后调用底层的 `FileMemoryStore::create` 或 `update`，并如之前讨论的，同步触发 SQLite 索引的更新。
* **Test Case & Acceptance Criteria**: LLM 只需输出简单的 JSON，Rust 引擎会自动在 `~/.gasket/memory/knowledge/` 下生成包含完美 `---` 边界和正确时间戳的 Markdown 文件。

#### Task 3: 降级或砍掉 `HistorySearchTool` (Kill the DBA Tool)
* **What**: 强烈建议直接**删除** `HistorySearchTool`。如果非要留，就将它降级为只搜索 `session_summaries`。
* **Why**: 减少 LLM 工具的数量可以显著降低 Prompt 复杂度，提高模型遵循指令的成功率（KISS 原则）。历史记录的检索应该由系统通过 RAG（如你已经实现的 `HistoryRecallHook`）隐式地完成，而不是让 LLM 显式地调用工具。
* **Where**: `gasket/engine/src/tools/history_search.rs`。
* **How**:
  1. 删掉这个文件。
  2. 从 `ToolRegistry` 里移除它的注册。
  3. 依靠 `AgentLoop` 里的 `HistoryRecallHook` (自动把当前的 query 拿去算向量，找相似的旧消息注入到 prompt 里)。这就足够了。
* **Test Case & Acceptance Criteria**: 系统不再向 LLM 暴露 `history_search` 工具。LLM 通过隐式的上下文注入依然能回答“我们上次聊这个话题时你说了什么”的问题。

---

**总结：**

在设计 LLM Tool 时，永远记住：**工具应该是业务逻辑的高级封装，而不是底层系统的 API 暴露。**

- 提供 `MemorySearch`（走向量库）和 `Memorize`（帮你写 YAML），这是在给 LLM 提供一个**功能完整的大脑海马体**。
- 提供 `HistorySearch`（走 SQL LIKE），这是在逼 LLM 兼职做你的**运维工程师**。

砍掉不必要的工具，把该封装的格式封装在 Rust 侧。去执行吧。