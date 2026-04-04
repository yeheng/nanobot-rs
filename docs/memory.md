# 记忆系统

> Gasket 长期记忆系统 — 基于场景的多维记忆存储与检索

---

## 概述

Gasket 记忆系统为个人 AI 助手提供跨会话的长期记忆能力。系统采用 **场景 (Scenario) x 频率 (Frequency)** 二维模型组织记忆，以纯 Markdown 文件为存储载体，支持人类直接编辑。

**核心特性：**

- **人类可编辑** — 每条记忆是独立的 `.md` 文件，任何文本编辑器可直接修改
- **懒加载** — 三阶段加载策略，硬性 Token 上限防止上下文爆炸
- **自动分层** — 根据访问频率自动调整加载优先级
- **语义搜索** — 嵌入向量搜索跨越场景边界连接相关记忆
- **版本历史** — 每次修改自动保存历史版本
- **去重检测** — 定时扫描发现潜在重复记忆

---

## 目录结构

```
~/.gasket/memory/
├── profile/                    # 用户身份与偏好
│   ├── _INDEX.md               # 自动生成的索引
│   ├── preferences.md          # 偏好设置
│   └── background.md           # 背景信息
├── active/                     # 当前工作与焦点
│   ├── _INDEX.md
│   ├── current.md              # 当前聚焦内容
│   └── backlog.md              # 待办事项
├── knowledge/                  # 学到的知识
│   ├── _INDEX.md
│   └── rust-async-patterns.md
├── decisions/                  # 决策与理由
│   ├── _INDEX.md
│   └── chose-sqlite.md
├── episodes/                   # 经历与事件
│   ├── _INDEX.md
│   └── fixed-compactor-bug.md
└── reference/                  # 外部参考资料
    ├── _INDEX.md
    └── useful-links.md
```

---

## 六大场景

| 场景 | 用途 | 加载时机 | Token 预算 |
|------|------|---------|-----------|
| **profile** | 用户身份、偏好、沟通风格 | 每次会话必加载 | ~200 |
| **active** | 当前工作焦点、待办事项 | 每次会话必加载 | ~500 |
| **knowledge** | 学到的概念、模式、约定 | 按主题匹配加载 | ~1000 |
| **decisions** | 做出的选择及其理由 | 决策场景或语义搜索 | ~1000 |
| **episodes** | 经历、事件及其结果 | 主要通过语义搜索 | 按需 |
| **reference** | 外部链接、联系人、工具 | 显式请求或语义搜索 | 按需 |

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
---

选择 SQLite 作为主存储后端的原因：

- 单用户桌面应用，无并发需求
- 零外部依赖，通过 rusqlite 捆绑
- FTS5 提供内置全文搜索
- 事件溯源模式天然适配追加式表结构
```

### 频率层级

| 层级 | 含义 | 加载策略 |
|------|------|---------|
| **hot** | 始终加载 | 场景激活时必定注入上下文 |
| **warm** | 按主题加载 | 标签匹配时注入上下文 |
| **cold** | 按需搜索 | 仅在显式搜索时加载 |
| **archived** | 历史归档 | 不主动加载，仅保留 |

### 标签规则

- 全小写，无空格，kebab-case（如 `rust-async`）
- 最多 10 个标签，每个最长 30 字符
- 保留前缀：`project:`、`session:`、`focus:`（系统自动生成）

---

## 三阶段加载策略

```
阶段 1: 引导加载 (~700 tokens, 必定执行)
┌────────────────────────────────────────┐
│ profile/*.md  (所有用户身份文件)         │  ~200 tokens
│ active/current.md  (当前焦点)           │  ~200 tokens
│ active/backlog.md   (待办事项)          │  ~250 tokens
└────────────────────────────────────────┘

阶段 2: 场景感知 (~1500 tokens, 根据行为推断)
┌────────────────────────────────────────┐
│ 读取场景 _INDEX.md → 过滤 hot 项 → 加载  │
│ 然后加载匹配标签的 warm 项              │
└────────────────────────────────────────┘

阶段 3: 按需搜索 (~1000 tokens, 根据查询)
┌────────────────────────────────────────┐
│ 标签搜索 + 嵌入向量搜索 → 合并排序       │
│ 加载 Top-K 结果直到预算耗尽             │
└────────────────────────────────────────┘

硬性上限: 3200 tokens
```

### 评分合并算法

标签搜索与嵌入搜索的结果通过归一化评分合并：

```
TAG_WEIGHT = 0.4, EMBEDDING_WEIGHT = 0.6

tag_score = 匹配标签数 / 查询标签数        → [0.0, 1.0]
emb_score = (cosine_similarity + 1.0) / 2.0 → [0.0, 1.0]
merged = tag_score * 0.4 + emb_score * 0.6
```

---

## 频率生命周期

### 自动衰减

```
hot  → 7 天未访问 → warm
warm → 30 天未访问 → cold
cold → 90 天未访问 → archived
```

**豁免场景：** Profile 始终为 hot，永不衰减。

### 自动提升

```
cold → 被访问（标签匹配或语义命中） → warm
warm → 7 天内访问 3 次以上 → hot
```

### 访问追踪

采用延迟批量写入，避免写放大：

1. 每次加载记忆时，追加到内存访问日志
2. 不立即写入磁盘
3. 满足以下条件时批量刷新：日志超过 50 条 / 会话结束 / 每 5 分钟

---

## 人类编辑

### 可编辑元素

- 任何记忆 `.md` 文件（前置元数据 + 内容）
- `_INDEX.md` 中的 `<!-- HUMAN_NOTES_START -->` 到 `<!-- HUMAN_NOTES_END -->` 区域
- `active/current.md` 和 `active/backlog.md`

### 自动生成元素（重新生成时覆盖）

- `_INDEX.md` 的表格区域
- `_INDEX.md` 的 `<!-- -->` 头部注释
- 前置元数据中的 `access_count`、`last_accessed`、`tokens` 字段

### 冲突解决

- 用户编辑优先：`title`、`tags`、`frequency`、`type`
- 系统管理优先：`access_count`、`last_accessed`、`tokens`、`updated`

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

每周自动运行跨会话去重扫描：

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

系统监控 `~/.gasket/memory/` 目录变化（需启用 `memory-watcher` 特性）：

- **防抖策略：** 最后一次写入后等待 2 秒再处理
- **忽略项：** `.history/` 目录、`.tmp` 文件、`_INDEX.md`
- **事件处理：** 新建 → 嵌入 + 索引；修改 → 重嵌入 + 更新；删除 → 清理 + 重索引

---

## 数据库表

记忆系统在 SQLite 中使用两个辅助表：

```sql
-- 嵌入向量存储（语义搜索）
memory_embeddings (
    memory_path TEXT PRIMARY KEY,
    scenario    TEXT NOT NULL,
    tags        TEXT,           -- JSON 数组
    frequency   TEXT NOT NULL DEFAULT 'warm',
    embedding   BLOB NOT NULL,  -- f32 向量
    token_count INTEGER NOT NULL,
    created_at  TIMESTAMP,
    updated_at  TIMESTAMP
)

-- 去重报告
dedup_reports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_a    TEXT NOT NULL,
    memory_b    TEXT NOT NULL,
    similarity  REAL NOT NULL,
    suggestion  TEXT NOT NULL,   -- merge | supersede | keep-both
    created_at  TIMESTAMP,
    resolved    BOOLEAN DEFAULT FALSE
)
```

---

## 代码模块

### Storage 层 (`gasket/storage/src/memory/`)

| 文件 | 职责 |
|------|------|
| `types.rs` | 核心类型：Scenario、Frequency、MemoryMeta、TokenBudget |
| `frontmatter.rs` | YAML 前置元数据解析与序列化 |
| `path.rs` | 路径解析：基础目录、场景目录、索引路径 |
| `store.rs` | FileMemoryStore — 文件系统 CRUD 与版本历史 |
| `index.rs` | FileIndexManager — _INDEX.md 生成与解析（原子写入） |
| `embedding_store.rs` | EmbeddingStore — SQLite 嵌入向量存储与检索 |
| `retrieval.rs` | RetrievalEngine — 标签 + 嵌入搜索 + 归一化合并评分 |
| `lifecycle.rs` | AccessLog + FrequencyManager — 频率衰减/提升 + 批量访问追踪 |
| `watcher.rs` | MemoryWatcher — 文件监控与防抖（feature-gated） |
| `dedup.rs` | DedupScanner — 跨会话去重扫描 |

### Engine 层 (`gasket/engine/src/agent/`)

| 文件 | 职责 |
|------|------|
| `memory_manager.rs` | MemoryManager 门面 — 三阶段加载 + Token 预算执行 |

### Agent Loop 集成

MemoryManager 在 Agent Loop 的 `prepare_pipeline()` 中注入：

```
用户消息 → [系统提示] → [记忆加载] → [历史处理] → [摘要注入] → [组装提示] → LLM
                         ↑
              MemoryManager.load_for_context()
              三阶段加载 + 3200 token 硬上限
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
