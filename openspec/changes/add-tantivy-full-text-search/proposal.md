# Change: Add Tantivy Full-Text Search for Memory and History

## Why

当前项目的记忆 (memory) 和会话历史 (history) 都存储在 SQLite 中，并使用 FTS5 进行全文检索。虽然 SQLite FTS5 功能完备，但存在以下局限：

1. **检索能力有限**：FTS5 的查询语法相对简单，不支持复杂的布尔查询、模糊匹配、同义词扩展等高级检索功能
2. **性能瓶颈**：随着数据量增长，SQLite FTS5 的检索性能会下降，尤其是在并发查询场景下
3. **相关性排序单一**：FTS5 的相关性评分算法不够灵活，无法根据业务场景定制排序策略
4. **LLM 检索体验不佳**：当前 `memory_search` 工具仅支持简单的关键词匹配，LLM 难以精准检索到相关记忆

[Tantivy](https://github.com/quickwit-oss/tantivy) 是 Rust 编写的高性能全文检索引擎，功能对标 Lucene/Elasticsearch，能够提供：
- 更丰富的查询语法（布尔查询、短语查询、模糊查询、通配符等）
- 更灵活的相关性评分（BM25、TF-IDF 可定制）
- 更好的并发读取性能
- 支持分词器定制（中文分词等）

## What Changes

### 新增功能

1. **添加 Tantivy 依赖** - 在 `nanobot-core/Cargo.toml` 中添加 `tantivy` crate

2. **创建 Tantivy 索引目录** - 实现 `TantivyIndex` 结构，管理记忆和历史的倒排索引

3. **实现 MemoryTantivySearchTool** - 新增工具供 LLM 调用，支持：
   - 布尔查询（AND/OR/NOT）
   - 短语查询（精确匹配）
   - 模糊查询（拼写容错）
   - 通配符查询
   - 相关性评分排序
   - 结果高亮（可选）

4. **实现 HistoryTantivySearchTool** - 新增工具供 LLM 检索会话历史

5. **索引同步机制** - 当记忆/历史新增、更新、删除时，同步更新 Tantivy 索引

6. **索引持久化** - Tantivy 索引文件存储在 `~/.nanobot/tantivy-index/`

### 架构调整

- 保留现有 SQLite FTS5 作为备份检索机制
- Tantivy 作为主检索引擎，提供更强大的查询能力
- 通过 trait 抽象检索接口，便于未来扩展

### 配置项

在配置文件中添加：
```yaml
search:
  tantivy:
    enabled: true
    index_path: ~/.nanobot/tantivy-index
    memory_index:
      tokenizer: "default"  # 或 "chinese" 等
    history_index:
      tokenizer: "default"
```

## Impact

### Affected specs

- **memory-search** - 新增记忆检索能力
- **history-search** - 新增历史检索能力（新能力）
- **agent-tools** - 新增可供 Agent 调用的工具

### Affected code

**新增文件**:
- `nanobot-rs/nanobot-core/src/search/tantivy/mod.rs` - Tantivy 索引管理模块
- `nanobot-rs/nanobot-core/src/search/tantivy/memory_index.rs` - 记忆索引实现
- `nanobot-rs/nanobot-core/src/search/tantivy/history_index.rs` - 历史索引实现
- `nanobot-rs/nanobot-core/src/tools/memory_tantivy_search.rs` - 记忆检索工具
- `nanobot-rs/nanobot-core/src/tools/history_tantivy_search.rs` - 历史检索工具

**修改文件**:
- `nanobot-rs/nanobot-core/Cargo.toml` - 添加 tantivy 依赖
- `nanobot-rs/nanobot-core/src/lib.rs` - 导出新模块
- `nanobot-rs/nanobot-core/src/tools/mod.rs` - 注册新工具
- `nanobot-rs/nanobot-core/src/agent/loop_.rs` - 集成新工具到 Agent
- `nanobot-rs/nanobot-core/src/memory/sqlite/memories.rs` - 添加索引同步钩子
- `nanobot-rs/nanobot-core/src/session/manager.rs` - 添加索引同步钩子

### 配置变更

- `config.example.yaml` - 添加 search.tantivy 配置段

### 向后兼容性

- **非破坏性变更** - 现有 `memory_search` 工具保留，基于 Tantivy 的新工具作为补充
- 现有 SQLite FTS5 索引继续工作，Tantivy 索引作为增强层
- API 兼容，现有功能不受影响
