## 1. 项目准备
- [x] 1.1 读取 `nanobot-rs/nanobot-core/Cargo.toml` 了解当前依赖结构
- [x] 1.2 添加 `tantivy = "0.25"` 依赖到 `nanobot-core/Cargo.toml`
- [x] 1.3 创建 `nanobot-rs/nanobot-core/src/search/` 目录结构

## 2. Tantivy 索引核心实现
- [x] 2.1 实现 `TantivyIndex` 基础结构（`search/tantivy/mod.rs`）
  - [x] 2.1.1 定义 TantivyError 错误类型
  - [x] 2.1.2 实现 From 转换
- [x] 2.2 实现 `MemoryIndex`（`search/tantivy/memory_index.rs`）
  - [x] 2.2.1 定义 Memory Schema（id, content, title, tags, file_path, modified_at, created_at）
  - [x] 2.2.2 实现 `index_document()` 方法
  - [x] 2.2.3 实现 `delete_document()` 方法
  - [x] 2.2.4 实现 `search()` 方法
  - [x] 2.2.5 实现 `rebuild()` 方法
- [x] 2.3 实现 `HistoryIndex`（`search/tantivy/history_index.rs`）
  - [x] 2.3.1 定义 History Schema（id, content, role, session_key, timestamp, tools）
  - [x] 2.3.2 实现 `index_document()` 方法
  - [x] 2.3.3 实现 `delete_document()` 方法
  - [x] 2.3.4 实现 `delete_session()` 方法
  - [x] 2.3.5 实现 `search()` 方法

## 3. 查询接口实现
- [x] 3.1 定义查询类型（`search/query.rs`）
  - [x] 3.1.1 `SearchQuery` 结构体
  - [x] 3.1.2 `BooleanQuery` 结构体
  - [x] 3.1.3 `FuzzyQuery` 结构体
  - [x] 3.1.4 `DateRange` 结构体
  - [x] 3.1.5 `SortOrder` 枚举
- [x] 3.2 实现搜索结果结构（`search/result.rs`）
  - [x] 3.2.1 `SearchResult` 结构体
  - [x] 3.2.2 `HighlightedText` 辅助结构

## 4. 工具实现
- [x] 4.1 实现 `MemoryTantivySearchTool`（`tools/memory_tantivy_search.rs`）
  - [x] 4.1.1 定义工具参数 schema（LLM 调用）
  - [x] 4.1.2 实现 `execute` 方法
  - [x] 4.1.3 实现结果格式化
  - [x] 4.1.4 编写单元测试
- [x] 4.2 实现 `HistoryTantivySearchTool`（`tools/history_tantivy_search.rs`）
  - [x] 4.2.1 定义工具参数 schema
  - [x] 4.2.2 实现 `execute` 方法
  - [x] 4.2.3 实现结果格式化
  - [x] 4.2.4 编写单元测试

## 5. Agent 集成
- [x] 5.1 修改工具注册
  - [x] 5.1.1 在 agent.rs 中添加 `MemoryTantivySearchTool`
  - [x] 5.1.2 在 agent.rs 中添加 `HistoryTantivySearchTool`

## 6. 验证与清理
- [x] 6.1 运行 `cargo clippy --all-features` 检查代码质量
- [x] 6.2 运行 `cargo test --all-features` 确保测试通过
- [x] 6.3 清理未使用的导入和警告
