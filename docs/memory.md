# 记忆系统

> Gasket 长期记忆系统 — 基于场景的多维记忆存储与检索

---

## 概述

Gasket 记忆系统为个人 AI 助手提供跨会话的长期记忆能力。系统采用 **场景 (Scenario) x 频率 (Frequency)** 二维模型组织记忆，以纯 Markdown 文件为存储载体（SSOT），通过 SQLite 元数据索引加速查询，支持人类直接编辑。

**核心特性：**

- **人类可编辑** — 每条记忆是独立的 `.md` 文件，任何文本编辑器可直接修改
- **懒加载** — 三阶段加载策略，硬性 Token 上限防止上下文爆炸
- **自动分层** — 根据访问频率自动调整加载优先级
- **语义搜索** — 嵌入向量搜索跨越场景边界连接相关记忆
- **Write-Through 一致性** — 写入操作同步更新文件系统（SSOT）和 SQLite 元数据/嵌入索引
- **版本历史** — 每次修改自动保存历史版本
- **SQLite 元数据索引** — 替代旧的 `_INDEX.md` 物化视图，支持 `json_each` 精确标签匹配

---

## 目录结构

```
~/.gasket/memory/
├── profile/                    # 用户身份与偏好
│   ├── preferences.md          # 偏好设置
│   └── background.md           # 背景信息
├── active/                     # 当前工作与焦点
│   ├── current.md              # 当前聚焦内容
│   └── backlog.md              # 待办事项
├── knowledge/                  # 学到的知识
│   └── rust-async-patterns.md
├── decisions/                  # 决策与理由
│   └── chose-sqlite.md
├── episodes/                   # 经历与事件
│   └── fixed-compactor-bug.md
├── reference/                  # 外部参考资料
│   └── useful-links.md
└── .history/                   # 版本历史（自动维护）
    ├── knowledge/
    │   ├── rust-async.2026-04-03T10-00-00.md
    │   └── rust-async.2026-04-04T15-30-00.md
    └── decisions/
        └── chose-sqlite.2026-04-01T08-00-00.md
```

> **注意:** 旧的 `_INDEX.md` 物化视图文件已被移除。所有元数据查询通过 SQLite `memory_metadata` 表完成，由 `MetadataStore` 模块驱动。

---

## 六大场景

| 场景 | 用途 | 加载时机 | 衰减豁免 | Token 预算 |
|------|------|---------|---------|-----------|
| **profile** | 用户身份、偏好、沟通风格 | 每次会话必加载 | 是（永不衰减） | ~200 |
| **active** | 当前工作焦点、待办事项 | 每次会话必加载 | 否 | ~500 |
| **knowledge** | 学到的概念、模式、约定 | 按主题匹配加载 | 否 | ~1000 |
| **decisions** | 做出的选择及其理由 | 决策场景或语义搜索 | 是（永不衰减） | ~1000 |
| **episodes** | 经历、事件及其结果 | 主要通过语义搜索 | 否 | 按需 |
| **reference** | 外部链接、联系人、工具 | 显式请求或语义搜索 | 是（永不衰减） | 按需 |

---

## 记忆文件格式

每条记忆是带有 YAML 前置元数据的 Markdown 文件：

```markdown
---
id: mem_0192456c-1a2b-7def-8901-2b3c4d5e6f70
title: "选择 SQLite 作为主存储后端"
type: decision
scenario: decisions
tags: [gasket, database, sqlite, architecture]
frequency: warm
access_count: 12
created: 2026-04-01T10:00:00Z
updated: 2026-04-03T15:30:00Z
last_accessed: 2026-04-03T15:30:00Z
auto_expire: false
expires: null
tokens: 180
superseded_by: null
---

选择 SQLite 作为主存储后端的原因：

- 单用户桌面应用，无并发需求
- 零外部依赖，通过 rusqlite 捆绑
- FTS5 提供内置全文搜索
- 事件溯源模式天然适配追加式表结构
```

### 频率层级

| 层级 | 含义 | 加载策略 | 搜索权重 |
|------|------|---------|---------|
| **hot** | 始终加载 | 场景激活时必定注入上下文 | ×1.2 |
| **warm** | 按主题加载 | 标签匹配时注入上下文 | ×1.1 |
| **cold** | 按需搜索 | 仅在显式搜索时加载 | ×1.0 |
| **archived** | 历史归档 | 不主动加载，仅保留 | ×0.0（排除） |

### 标签规则

- 全小写，无空格，kebab-case（如 `rust-async`）
- 最多 10 个标签，每个最长 30 字符
- 保留前缀：`project:`、`session:`、`focus:`（系统自动生成）
- 使用 SQLite `json_each` 进行精确数组元素匹配，不使用 `LIKE` 子串扫描

---

## 三阶段加载策略

```
阶段 1: 引导加载 (~700 tokens, 必定执行)
┌────────────────────────────────────────┐
│ profile/*.md  (所有用户身份文件)         │  ~200 tokens
│ active/*.md   (Hot 优先，然后 Warm)     │  ~500 tokens
│             跳过 Cold 和 Archived       │
└────────────────────────────────────────┘
         ↓ 由 MetadataStore SQLite 查询驱动
         ↓ 按频率排序: Hot → Warm → Cold

阶段 2: 场景感知 (~1500 tokens, 根据行为推断)
┌────────────────────────────────────────┐
│ SQLite 查询 hot 项（不检查标签）         │
│ SQLite 查询 warm 项（标签匹配过滤）      │
└────────────────────────────────────────┘
         ↓ SQL json_each EXISTS 子查询

阶段 3: 按需搜索 (~1000 tokens, 根据查询)
┌────────────────────────────────────────┐
│ RetrievalEngine.search() 组合搜索       │
│ → 加载 Top-K 结果直到预算耗尽           │
│ → 跳过已在阶段 1/2 加载的文件           │
└────────────────────────────────────────┘

硬性上限: 3200 tokens
         ↓ 超出时按加载顺序截断并重新计算分阶段明细
```

### 搜索评分算法

检索引擎采用 **嵌入主评分 + 标签硬过滤 + 频率加权** 策略：

```
1. 嵌入相似度搜索: cosine_similarity → 归一化到 [0.0, 1.0]
2. 标签作为硬过滤: 查询有标签时，结果必须匹配至少一个
3. 频率加权:
   - Hot:  emb_score × 1.2
   - Warm: emb_score × 1.1
   - Cold: emb_score × 1.0
   - Archived: 排除

final_score = embedding_score × frequency_bonus
```

**降级策略：** 若查询有标签但无嵌入结果，自动降级为纯标签搜索（`MetadataStore.query_by_tags()`）。

---

## Write-Through 一致性

所有 Agent 写入操作（创建、更新、删除）同步更新文件系统和 SQLite：

```
Agent 写入操作流程:
┌─────────────────────────────────────────────────────────────┐
│ create_memory()                                             │
│   1. 生成 UUID v7 ID + YAML frontmatter                    │
│   2. atomic_write() 到场景目录（tmp + rename）               │
│   3. 读取 file_mtime                                        │
│   4. upsert 到 memory_metadata 表                           │
│   5. upsert 到 memory_embeddings 表                          │
├─────────────────────────────────────────────────────────────┤
│ update_memory()                                             │
│   1. 读取现有文件，保留元数据                                 │
│   2. 更新 updated / last_accessed / tokens 字段              │
│   3. 保存旧版本到 .history/ + 裁剪                           │
│   4. atomic_write() 新内容                                  │
│   5. upsert 到 memory_metadata + memory_embeddings          │
├─────────────────────────────────────────────────────────────┤
│ delete_memory()                                             │
│   1. 删除文件                                                │
│   2. 从 memory_metadata 删除                                │
│   3. 从 memory_embeddings 删除                               │
└─────────────────────────────────────────────────────────────┘
```

文件监控器使用 `file_mtime` 比较来检测外部编辑，避免重复处理。

---

## 频率生命周期

### 自动衰减

```
hot  → 7 天未访问 → warm
warm → 30 天未访问 → cold
cold → 90 天未访问 → archived
```

**豁免场景：** Profile、Decisions、Reference 始终为 hot，永不衰减。

**SQL 驱动衰减：** `MetadataStore.get_decay_candidates()` 直接查询 SQLite 中 `last_accessed` 超过阈值的条目，仅读写 O(k) 个文件而非 O(N) 全量扫描。

### 自动提升

```
cold → 被访问（任何访问） → warm
warm → 单次刷新中访问 3 次以上 → hot
```

### 访问追踪

采用延迟批量写入，避免写放大：

1. 每次加载记忆时，追加到内存 `AccessLog`
2. 不立即写入磁盘
3. 满足以下条件时批量刷新：日志超过 50 条 / 会话结束 / 每 5 分钟
4. 刷新时按 (scenario, filename) 分组，O(1) SQLite upsert

---

## 人类编辑

### 可编辑元素

- 任何记忆 `.md` 文件（前置元数据 + 内容）
- `active/` 目录下的文件
- 文件名可以是任意名称（不再限于 `current.md` / `backlog.md`）

### 系统管理字段（刷新时覆盖）

- `access_count`、`last_accessed`、`tokens` 字段
- `updated` 时间戳
- `frequency` 字段（由衰减/提升逻辑管理）
- SQLite `memory_metadata` 和 `memory_embeddings` 表中的所有数据

### 冲突解决

- 用户编辑优先：`title`、`tags`、`type`、`content`
- 系统管理优先：`access_count`、`last_accessed`、`tokens`、`updated`、`frequency`

### 版本历史

每次修改记忆文件时，系统自动将前一版本保存到 `~/.gasket/memory/.history/`：

```
.history/
├── knowledge/
│   ├── rust-async.2026-04-03T10-00-00.md
│   └── rust-async.2026-04-04T15-30-00.md
└── decisions/
    └── chose-sqlite.2026-04-01T08-00-00.md
```

每个文件最多保留 10 个历史版本，超出自动裁剪最旧的版本。

---

## 去重检测

跨会话去重扫描（通过 `EmbeddingStore.get_all_for_scenario()`）：

1. 收集每个场景的所有嵌入向量
2. 计算场景内两两余弦相似度
3. 标记相似度 > 0.85 的记忆对
4. 生成去重报告供 Agent 审阅

**建议策略：**

- 相似度 > 0.95 → 建议 "合并"
- 相似度 > 0.90 → 建议 "替代"（较新的替代较旧的）
- 相似度 0.85–0.90 → 建议 "保留两者"

**永不自动合并** — 所有操作需用户确认。

---

## 文件监控

系统监控 `~/.gasket/memory/` 目录变化：

- **防抖策略：** 最后一次写入后等待 2 秒再处理
- **mtime 比对：** 使用 `file_mtime` 字段检测外部编辑，避免重复处理
- **忽略项：** `.history/` 目录、`.tmp` 文件
- **事件处理：** 新建 → 嵌入 + upsert 元数据；修改 → 重嵌入 + 更新；删除 → 清理 + 删除元数据
- **自动索引：** `AutoIndexHandler` 在文件变化时同步更新 SQLite 元数据

---

## 数据库表

记忆系统在 SQLite 中使用三个辅助表：

```sql
-- 文件元数据索引（替代旧的 _INDEX.md）
memory_metadata (
    id          TEXT NOT NULL,
    path        TEXT NOT NULL,          -- 文件名（如 mem_xxx.md）
    scenario    TEXT NOT NULL,          -- 场景目录名
    title       TEXT NOT NULL DEFAULT '',
    memory_type TEXT NOT NULL DEFAULT 'note',
    frequency   TEXT NOT NULL DEFAULT 'warm',
    tags        TEXT NOT NULL DEFAULT '[]',  -- JSON 数组，json_each 查询
    tokens      INTEGER NOT NULL DEFAULT 0,
    updated     TEXT NOT NULL DEFAULT '',
    last_accessed TEXT NOT NULL DEFAULT '',
    file_mtime  BIGINT NOT NULL DEFAULT 0, -- 文件修改时间（纳秒）
    PRIMARY KEY (scenario, path)
)

-- 嵌入向量存储（语义搜索）
memory_embeddings (
    memory_path TEXT PRIMARY KEY,
    scenario    TEXT NOT NULL,
    tags        TEXT,                   -- JSON 数组
    frequency   TEXT NOT NULL DEFAULT 'warm',
    embedding   BLOB NOT NULL,          -- f32 向量
    token_count INTEGER NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
)
```

---

## 代码模块

### Storage 层 (`gasket/storage/src/memory/`)

| 文件 | 职责 |
|------|------|
| `types.rs` | 核心类型：Scenario、Frequency、MemoryMeta、MemoryFile、MemoryQuery、MemoryHit、TokenBudget |
| `frontmatter.rs` | YAML 前置元数据解析与序列化 |
| `path.rs` | 路径解析：基础目录、场景目录 |
| `store.rs` | FileMemoryStore — 文件系统 CRUD、原子写入、版本历史（最多 10 版本） |
| `index.rs` | FileIndexManager — 目录扫描器，解析 frontmatter 返回 MemoryIndexEntry（不生成 _INDEX.md） |
| `metadata_store.rs` | MetadataStore — SQLite 元数据索引，支持 json_each 精确标签匹配和 SQL 驱动衰减 |
| `embedding_store.rs` | EmbeddingStore — SQLite 嵌入向量存储与余弦相似度检索 |
| `retrieval.rs` | RetrievalEngine — 嵌入主评分 + 标签硬过滤 + 频率加权搜索 |
| `lifecycle.rs` | AccessLog + FrequencyManager — 频率衰减/提升 + 批量访问追踪 + SQL 驱动衰减 |
| `watcher.rs` | MemoryWatcher — 文件监控与防抖 + AutoIndexHandler mtime 比对 |

### Engine 层 (`gasket/engine/src/agent/`)

| 文件 | 职责 |
|------|------|
| `memory_manager.rs` | MemoryManager 门面 — 三阶段加载 + Write-Through CRUD + Token 预算执行 |
| `memory_provider.rs` | MemoryProvider trait — 解耦 HistoryCoordinator 与具体实现的查询接口 |
| `memory.rs` | MemoryStore — SqliteStore 薄包装，用于机器状态（会话、摘要、定时任务） |

### Agent Loop 集成

MemoryManager 在 Agent Loop 的 `prepare_pipeline()` 中注入：

```
用户消息 → [系统提示] → [记忆加载] → [历史处理] → [摘要注入] → [组装提示] → LLM
                         ↑
              MemoryManager.load_for_context()
              三阶段加载 + 3200 token 硬上限
              由 MemoryProvider trait 解耦
```

### MemoryProvider Trait

```rust
pub trait MemoryProvider: Send + Sync {
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext>;
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>>;
    async fn update_from_event(&self, _event: &SessionEvent) -> Result<()>;
    async fn create_memory(&self, scenario, filename, title, tags, frequency, content) -> Result<()>;
    async fn update_memory(&self, scenario, filename, content) -> Result<()>;
    async fn delete_memory(&self, scenario, filename) -> Result<()>;
}
```

---

## 特性标志

| 标志 | Crate | 用途 |
|------|-------|------|
| `memory-watcher` | storage | 启用文件监控（依赖 notify crate） |
| `local-embedding` | storage/engine | 启用本地 ONNX 嵌入（依赖 fastembed） |

---

## 配置

记忆系统无需额外配置。只要 `~/.gasket/memory/` 目录存在，系统自动激活。若目录不存在，Agent 以纯会话模式运行（无长期记忆）。

初始化记忆目录：

```bash
# 手动创建
mkdir -p ~/.gasket/memory/{profile,active,knowledge,decisions,episodes,reference}

# 或通过 Agent 首次写入记忆时自动创建
```

全量重建索引（修复陈旧索引）：

```bash
# CLI 命令
cargo run --release --package gasket-cli -- memory reindex
```
