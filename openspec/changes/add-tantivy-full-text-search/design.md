## Context

本项目需要在 SQLite 存储的基础上，添加高性能的全文检索能力，供 LLM Agent 调用。

**技术约束**:
- 必须使用 Rust 实现（项目技术栈）
- 需要支持中文分词（未来需求）
- 索引需要持久化，重启后可恢复
- 检索延迟应 < 100ms（单次查询）
- 支持并发读取

**相关方**:
- 终端用户：通过 Agent 间接使用检索功能
- LLM Agent：直接调用检索工具
- 开发者：维护和扩展检索功能

## Goals / Non-Goals

### Goals
- [x] 集成 Tantivy 作为全文检索引擎
- [x] 为 Memory 和 History 分别建立 Tantivy 索引
- [x] 实现 MemoryTantivySearchTool 供 Agent 调用
- [x] 实现 HistoryTantivySearchTool 供 Agent 调用
- [x] 索引与 SQLite 数据同步（增删改触发索引更新）
- [x] 支持布尔查询、模糊查询、短语查询
- [x] 支持相关性评分排序

### Non-Goals
- [ ] 替换 SQLite FTS5（保留作为备份）
- [ ] 实现分布式检索（单机场景足够）
- [ ] 实时检索分析（不需要查询日志/热词统计）
- [ ] 向量检索/语义搜索（未来可能添加 embedding 支持）

## Decisions

### Decision 1: 使用 Tantivy 0.23（最新版）

**What**: 选择 `tantivy = "0.23"` 作为检索引擎

**Why**:
- 最新稳定版，API 成熟
- 支持所有需要的查询类型
- 纯 Rust 实现，无外部依赖
- 文档完善，社区活跃
- 性能优秀（对标 Lucene）

### Decision 2: 索引 Schema 设计

**Memory Index Schema**:
```rust
// 字段定义
text_field: TextField      // 记忆内容（全文检索，带 TF-IDF 评分）
tags_field: Vec<String>    // 标签（faceted 搜索）
source_field: String       // 来源（user/agent/system）
created_at: i64            // 创建时间（范围查询/排序）
updated_at: i64            // 更新时间（范围查询/排序）
memory_id: String          // 文档 ID（stored field）
```

**History Index Schema**:
```rust
// 字段定义
text_field: TextField      // 消息内容（全文检索）
role_field: String         // 角色（user/assistant/system/tool）
session_key: String        // 会话 ID（faceted 搜索）
timestamp: i64             // 时间戳（范围查询/排序）
tools_field: Vec<String>   // 使用的工具（可选）
message_id: String         // 文档 ID
```

### Decision 3: 分词器选择

**默认分词器**: `TextAnalyzerManager::default()`（英文友好）

**未来扩展**: 支持中文分词 via `tangram` 或 `jieba-rs`

配置化：
```yaml
search:
  tantivy:
    memory_index:
      tokenizer: "default"  # 或 "chinese"
    history_index:
      tokenizer: "default"
```

### Decision 4: 索引同步策略

**写时同步（Write-Through）**:
- SQLite 写入成功后，立即更新 Tantivy 索引
- 优点：索引与数据强一致
- 缺点：写入延迟略增（~10ms）

**实现**:
- `MemoryStore::save()` → 成功后调用 `TantivyIndex::add_document()`
- `MemoryStore::delete()` → 成功后调用 `TantivyIndex::delete_document()`
- `SessionManager::append_message()` → 成功后调用 `HistoryIndex::add_document()`

**批量重建**: 提供 CLI 命令 `nanobot search rebuild` 用于全量重建索引

### Decision 5: 查询 API 设计

```rust
pub struct TantivyQuery {
    pub text: Option<String>,      // 全文检索关键词
    pub boolean: Option<BooleanQuery>,  // 布尔查询
    pub fuzzy: Option<FuzzyQuery>,    // 模糊查询
    pub tags: Vec<String>,           // 标签过滤
    pub date_range: Option<DateRange>, // 时间范围
    pub limit: usize,
    pub offset: usize,
    pub sort: SortOrder,             // 相关性/时间
}

pub struct BooleanQuery {
    pub must: Vec<String>,   // 必须包含
    pub should: Vec<String>, // 可能包含
    pub not: Vec<String>,    // 必须不包含
}

pub struct FuzzyQuery {
    pub text: String,
    pub distance: u8,        // 编辑距离（默认 2）
    pub prefix: bool,        // 是否允许前缀匹配
}
```

### Decision 6: 工具参数设计（LLM 调用）

**MemoryTantivySearchTool 参数**:
```json
{
  "query": "用户上次提到的项目",      // 必填，全文检索
  "boolean": {                        // 可选，布尔查询
    "must": ["重要"],
    "not": ["草稿"]
  },
  "tags": ["decision", "lesson"],     // 可选，标签过滤
  "fuzzy": {                          // 可选，模糊查询
    "text": "projct",
    "distance": 1
  },
  "limit": 10,
  "sort": "relevance"                 // 或 "date"
}
```

**返回格式**:
```json
{
  "total": 3,
  "results": [
    {
      "memory_id": "uuid",
      "content": "记忆内容...",
      "score": 2.34,
      "tags": ["decision"],
      "updated_at": "2024-01-01T12:00:00Z",
      "highlight": "用户上次提到的<span class='highlight'>项目</span>"
    }
  ]
}
```

## Alternatives Considered

### Alternative 1: 使用 Elasticsearch

**方案**: 部署外部 ES 集群，通过 HTTP API 检索

**优点**:
- 功能最强大，支持分布式
- 支持向量检索、聚合分析

**缺点**:
- 需要外部服务，增加部署复杂度
- 网络延迟（~50ms+）
- 资源消耗大（JVM）
- 不适合个人/小型项目

**结论**: 过度设计，不适合本项目场景

### Alternative 2: 使用 Meilisearch

**方案**: 使用 Meilisearch（也是 Rust 编写）

**优点**:
- 开箱即用，配置简单
- 支持中文分词
-  typo 容错好

**缺点**:
- 需要独立服务进程
- 自定义查询语法较复杂

**结论**: 不适合嵌入式场景

### Alternative 3: 继续用 SQLite FTS5

**方案**: 不引入新依赖，优化现有 FTS5 使用

**优点**:
- 无新增依赖
- 架构简单

**缺点**:
- 查询能力有限
- 并发性能差
- 相关性排序不灵活

**结论**: 无法满足 LLM 精准检索需求

## Risks / Trade-offs

### Risk 1: 索引与数据不一致

**场景**: Tantivy 索引更新失败，但 SQLite 已提交

**缓解**:
- 写入时使用事务，索引失败则回滚 SQLite
- 提供 `rebuild-index` CLI 命令用于修复
- 启动时检查索引完整性

### Risk 2: 索引文件过大

**场景**: 长期运行后索引文件占用过多磁盘

**缓解**:
- 定期清理过期记忆（TTL）
- 支持索引压缩（Tantivy 支持）
- 监控索引大小，告警

### Risk 3: 中文分词效果差

**场景**: 默认分词器对中文支持不佳

**缓解**:
- 未来集成 `jieba-rs` 或 `tangram`
- 支持配置切换分词器
- 提供自定义词典

## Migration Plan

### Phase 1: 基础集成（本次变更）
1. 添加 Tantivy 依赖
2. 实现 MemoryTantivySearchTool
3. 实现 HistoryTantivySearchTool
4. 索引同步机制
5. 基本查询功能

### Phase 2: 增强功能（未来）
1. 中文分词支持
2. 高亮显示
3. 查询建议/自动补全
4. 检索分析（热门搜索、零结果查询）

### Phase 3: 性能优化（未来）
1. 索引压缩
2. 查询缓存
3. 并发查询优化

## Open Questions

1. **是否需要在配置中暴露索引路径？** - 建议默认 `~/.nanobot/tantivy-index/`

2. **是否需要支持多语言分词？** - 未来需求，先支持默认分词器

3. **是否需要支持向量检索？** - 未来可添加 embedding 支持，与 Tantivy 混合检索
