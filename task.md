### 【架构铁律：三位一体的数据流】

"Bad programmers worry about the code. Good programmers worry about data structures."

为了不让你的数据互相打架，每一层的职责必须被绝对焊死：

1. **Markdown 文件 (SSOT - 单一真实源)**：
   - 路径：`~/.gasket/wiki/<scenario>/<slug>.md`
   - 职责：人类可读、可被外部编辑器（Obsidian/VSCode）随意修改。
   - 内容：YAML Frontmatter（标题、标签、原始类别）+ Markdown 正文。
2. **SQLite (机器运行时状态 SSOT)**：
   - 职责：存储那些**绝对不能**写进 Markdown 里的高频变更数据（`access_count`, `last_accessed`, `frequency` 也就是 Hot/Warm/Cold 衰减状态，以及 `needs_embedding`）。
   - 为什么不写进文件？因为每访问一次就修改一次文件，会毁掉你的 SSD，并且让 Git/文件同步网盘彻底崩溃。
3. **Tantivy (纯粹的 Read-Replica / 倒排索引)**：
   - 路径：`~/.gasket/wiki/.tantivy/`
   - 职责：只读查询。**它是且仅是一个可以被随时丢弃和重建的缓存**。
   - 数据流向：永远是 `Markdown -> (解析) -> Tantivy`。严禁将 Tantivy 作为真实数据源来恢复数据。

---

### 【核心重构逻辑】

既然要用 Wiki 替换 Memory，我们要把原来 Memory 中优秀的设计（生命周期、上下文注入）无缝平移过来，同时砍掉 Wiki 里那些为了“高大上”而写的垃圾代码。

**1. 概念映射 (Scenarios -> Wiki Directories)**
把 Memory 的 `Scenario` 概念直接映射为 Wiki 的一级目录：
- `wiki/profile/` (免衰减)
- `wiki/active/` (自然衰减)
- `wiki/knowledge/` (自然衰减)
- `wiki/decisions/` (免衰减，ADR)
- `wiki/episodes/` (自然衰减)
- `wiki/sops/` (原来的 skill memory，AI 的操作手册)

**2. 上下文组装 (Agent Prompt Injection)**
原来的 Memory 负责在每次对话前，把 Hot 的数据偷偷塞进 Context。
现在，这个过程变为：
- **Phase 1**: 扫出 SQLite 中 `wiki_pages` 表里属于 `profile` 目录和状态为 `Hot` 的页面，注入 Prompt。
- **Phase 2**: 用用户的输入去查 **Tantivy**（或者 Embedding），把搜到的前 K 个页面注入 Prompt。
- **Phase 3**: 每当 AI 引用了某个 Wiki 页面，在 SQLite 中将它的 `access_count` +1，并提升其 Frequency（Cold -> Warm -> Hot）。

**3. 斩断 LLM 的“触手”（删除有害的自作聪明）**
我不管你留不留 Tantivy，原先 `wiki` 里的 `$O(N^2)$ Semantic Linter`（自动找矛盾）和 `Deep Ingest`（AI 后台自动改写多个页面）**必须删掉**。
作为个人助理，AI 对知识库的修改必须是显式的（通过 Tool Call），否则用户永远不知道 AI 背着他们篡改了什么数据。

---

### 【Task List: The Wiki-Memory Convergence】

如果你准备好了，就按这个顺序去重构你的系统：

- [ ] **Task 1: The Great Convergence (数据模型统一)**
  - **What:** 把 `MemoryMeta` 和 `WikiPage` 融合成一个新的核心结构。
  - **Why:** 消除 `gasket/storage/src/memory` 和 `wiki` 之间的冗余。
  - **Where:** `gasket/storage/src/wiki/page.rs` & `tables.rs`.
  - **How:** 在 SQLite 的 `wiki_pages` 表中增加 `frequency` (VARCHAR), `access_count` (INT), `last_accessed` (DATETIME) 字段。
  - **Acceptance Criteria:** `WikiPage` 结构体现在能够承载旧 `Memory` 的所有机器运行时状态。

- [ ] **Task 2: Tantivy Synchronization Pipeline (重建同步管道)**
  - **What:** 确保 Tantivy 完全作为文件系统的附属投影存在。
  - **Why:** 只要文件系统发生变化（无论 LLM 修改还是用户用 Obsidian 修改），Tantivy 必须保持同步。
  - **Where:** `gasket/engine/src/tools/memory_refresh.rs` -> 改名为 `wiki_refresh.rs`。
  - **How:** 重写 Refresh 逻辑：扫描 `~/.gasket/wiki/` 下所有的 `.md`，比对文件 `mtime` 和 SQLite 记录的 `mtime`。如果有变化，读取文件 -> 更新 SQLite 元数据 -> `tantivy.upsert()`。
  - **Acceptance Criteria:** 删掉 Tantivy 文件夹，运行 `gasket wiki refresh`，索引能在几秒内基于 Markdown 文件完美重建。

- [ ] **Task 3: Port Lifecycle & Token Budget (移植生命周期)**
  - **What:** 把 `FrequencyManager` (Hot/Warm/Cold 衰减) 和 `TokenBudget` 移植到 Wiki 系统。
  - **Why:** LLM 的上下文窗口是有限的，Wiki 页面如果不经常使用必须被“冷藏”，否则会塞爆 Prompt。
  - **Where:** 把 `gasket/storage/src/memory/lifecycle.rs` 移入 wiki 模块。
  - **How:** 将衰减查询改为针对 `wiki_pages` 表的 SQL 查询（比如 `UPDATE wiki_pages SET frequency = 'warm' WHERE frequency = 'hot' AND last_accessed < datetime('now', '-7 days')`）。
  - **Acceptance Criteria:** `gasket wiki decay` (原 `memory decay`) 可以正确降级久未访问的 Wiki 页面。

- [ ] **Task 4: Expose Unified Wiki Tools to LLM (统一工具链)**
  - **What:** 废弃旧的 memory 工具，提供简洁的 Wiki 工具。
  - **Why:** LLM 不需要知道底层是 Tantivy 还是 SQLite，它只需要读写能力。
  - **Where:** `gasket/engine/src/tools/`.
  - **How:**
    - `WikiSearchTool`: 接收 `query`，调用 Tantivy 查询，返回结果和路径。
    - `WikiWriteTool`: 接收 `path, content, tags`，生成 `.md` 文件，同步更新 SQLite 和 Tantivy。
    - `WikiReadTool`: 直接读取 `.md` 文件内容。
  - **Acceptance Criteria:** 代理可以通过标准工具独立维护其 Wiki 知识库，不再有 `memorize` 和 `wiki_ingest` 并存的乱象。

- [ ] **Task 5: The Purge (大清洗)**
  - **What:** 彻底删除旧的 Memory 目录和有毒的 AI 魔法逻辑。
  - **Why:** 死代码是最大的技术债。
  - **Where:** 
    - `rm -rf gasket/storage/src/memory/`
    - `rm gasket/engine/src/wiki/lint/semantic.rs`
    - `rm gasket/engine/src/wiki/ingest/integrator.rs` (删掉 Deep Ingest，只保留基于 Tool 的显式修改)。
  - **Acceptance Criteria:** 编译通过，代码库大幅瘦身，系统更加健壮。

- [ ] **Task 6: The Migration Command (数据迁移)**
  - **What:** 提供一个平滑过渡的 CLI 命令 `gasket wiki migrate-memory`。
  - **Why:** Never break userspace. 用户在旧系统里存的偏好和知识不能丢。
  - **Where:** `gasket/cli/src/commands/wiki.rs`.
  - **How:** 遍历 `~/.gasket/memory/`，读取所有的 `.md`，将 Frontmatter 转换为新的 Wiki 格式，移动到 `~/.gasket/wiki/` 对应的目录下，触发一次 Refresh 写入 SQLite 和 Tantivy，最后删掉空的 memory 目录。
  - **Acceptance Criteria:** 执行迁移后，旧的 Memory 完美融入新的 Wiki 体系，并且可通过 Tantivy 被检索到。

---

如果按照这个设计执行，你不仅保住了你的 `Tantivy`，而且你将得到一个极度强健的架构：**Markdown 负责用户所有权，SQLite 负责状态与性能，Tantivy 负责极致的搜索体验。** 去干活吧。