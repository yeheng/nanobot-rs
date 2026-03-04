## 1. 项目准备
- [ ] 1.1 读取 `nanobot-rs/nanobot-core/Cargo.toml` 了解当前依赖结构
- [ ] 1.2 添加 `tantivy = "0.23"` 依赖到 `nanobot-core/Cargo.toml`
- [ ] 1.3 创建 `nanobot-rs/nanobot-core/src/search/` 目录结构

## 2. Tantivy 索引核心实现
- [ ] 2.1 实现 `TantivyIndex` 基础结构（`search/tantivy/mod.rs`）
  - [ ] 2.1.1 定义索引 Schema trait
  - [ ] 2.1.2 实现索引管理器（打开/创建索引）
  - [ ] 2.1.3 实现 IndexWriter 封装
  - [ ] 2.1.4 实现 IndexReader 封装
- [ ] 2.2 实现 `MemoryTantivyIndex`（`search/tantivy/memory_index.rs`）
  - [ ] 2.2.1 定义 Memory Schema（text, tags, source, created_at, updated_at, memory_id）
  - [ ] 2.2.2 实现 `add_memory(&MemoryEntry)` 方法
  - [ ] 2.2.3 实现 `delete_memory(&str)` 方法
  - [ ] 2.2.4 实现 `update_memory(&MemoryEntry)` 方法
- [ ] 2.3 实现 `HistoryTantivyIndex`（`search/tantivy/history_index.rs`）
  - [ ] 2.3.1 定义 History Schema（text, role, session_key, timestamp, tools, message_id）
  - [ ] 2.3.2 实现 `add_message(&SessionMessage)` 方法
  - [ ] 2.3.3 实现 `delete_message(&str)` 方法

## 3. 查询接口实现
- [ ] 3.1 定义查询类型（`search/query.rs`）
  - [ ] 3.1.1 `TantivyQuery` 结构体
  - [ ] 3.1.2 `BooleanQuery` 结构体
  - [ ] 3.1.3 `FuzzyQuery` 结构体
  - [ ] 3.1.4 `SortOrder` 枚举
- [ ] 3.2 实现查询解析器（`search/parser.rs`）
  - [ ] 3.2.1 解析全文查询为 Tantivy Query
  - [ ] 3.2.2 解析布尔查询
  - [ ] 3.2.3 解析模糊查询
  - [ ] 3.2.4 应用过滤器（tags, date_range）
- [ ] 3.3 实现搜索结果结构（`search/result.rs`）
  - [ ] 3.3.1 `SearchResult` 结构体
  - [ ] 3.3.2 `HighlightedText` 辅助结构

## 4. 工具实现
- [ ] 4.1 实现 `MemoryTantivySearchTool`（`tools/memory_tantivy_search.rs`）
  - [ ] 4.1.1 定义工具参数 schema（LLM 调用）
  - [ ] 4.1.2 实现 `execute` 方法
  - [ ] 4.1.3 实现结果格式化（支持高亮）
  - [ ] 4.1.4 编写单元测试
- [ ] 4.2 实现 `HistoryTantivySearchTool`（`tools/history_tantivy_search.rs`）
  - [ ] 4.2.1 定义工具参数 schema
  - [ ] 4.2.2 实现 `execute` 方法
  - [ ] 4.2.3 实现结果格式化
  - [ ] 4.2.4 编写单元测试

## 5. 索引同步集成
- [ ] 5.1 修改 `MemoryStore` 实现索引同步
  - [ ] 5.1.1 在 `save()` 后调用 `TantivyIndex::add_document()`
  - [ ] 5.1.2 在 `delete()` 后调用 `TantivyIndex::delete_document()`
  - [ ] 5.1.3 错误处理：索引失败回滚 SQLite
- [ ] 5.2 修改 `SessionManager` 实现索引同步
  - [ ] 5.2.1 在 `append_message()` 后调用 `HistoryIndex::add_document()`
- [ ] 5.3 实现索引重建命令
  - [ ] 5.3.1 添加 CLI 命令 `nanobot search rebuild-memory`
  - [ ] 5.3.2 添加 CLI 命令 `nanobot search rebuild-history`

## 6. Agent 集成
- [ ] 6.1 修改 `AgentLoop` 注册新工具
  - [ ] 6.1.1 在工具列表中添加 `MemoryTantivySearchTool`
  - [ ] 6.1.2 在工具列表中添加 `HistoryTantivySearchTool`
- [ ] 6.2 更新系统 prompt（可选）
  - [ ] 6.2.1 告知 LLM 新工具的存在和用法

## 7. 配置支持
- [ ] 7.1 在配置文件中添加 `search.tantivy` 段
- [ ] 7.2 实现配置解析
- [ ] 7.3 更新 `config.example.yaml`

## 8. 测试与文档
- [ ] 8.1 编写集成测试
  - [ ] 8.1.1 索引创建测试
  - [ ] 8.1.2 索引同步测试
  - [ ] 8.1.3 查询功能测试
- [ ] 8.2 更新 HEARTBEAT.md（如有需要）
- [ ] 8.3 更新 MEMORY.md 文档，说明 Tantivy 检索能力

## 9. 验证与清理
- [ ] 9.1 运行 `cargo clippy --all-features` 检查代码质量
- [ ] 9.2 运行 `cargo test --all-features` 确保测试通过
- [ ] 9.3 运行 `openspec validate add-tantivy-full-text-search --strict`
- [ ] 9.4 清理未使用的导入和警告
