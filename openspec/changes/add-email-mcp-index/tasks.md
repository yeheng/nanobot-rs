## 1. 项目脚手架
- [ ] 1.1 创建 `nanobot-email-mcp` 目录结构
- [ ] 1.2 编写 `Cargo.toml`（包含 workspace 配置）
- [ ] 1.3 添加必要的 dependencies（tantivy、mcp-rs、tokio 等）

## 2. 核心功能实现
- [ ] 2.1 实现 EmailDocument 数据模型
- [ ] 2.2 实现 Tantivy 索引 schema（字段定义）
- [ ] 2.3 实现索引器（添加/更新文档）
- [ ] 2.4 实现查询引擎（搜索、过滤）

## 3. MCP 协议实现
- [ ] 3.1 实现 MCP tool: `index_emails` - 批量索引邮件
- [ ] 3.2 实现 MCP tool: `search_emails` - 搜索邮件
- [ ] 3.3 实现 MCP tool: `get_email` - 获取单封邮件
- [ ] 3.4 实现 MCP tool: `delete_email` - 删除索引
- [ ] 3.5 实现 MCP tool: `get_index_stats` - 索引统计
- [ ] 3.6 实现 MCP server 主循环（stdio transport）

## 4. CLI 工具
- [ ] 4.1 实现 `stats` 命令（查看索引统计）
- [ ] 4.2 实现 `search` 命令（搜索邮件）
- [ ] 4.3 实现 `clear` 命令（清空索引）

## 5. 配置和文档
- [ ] 5.1 编写 README.md
- [ ] 5.2 更新 workspace 配置

## 6. 测试
- [ ] 6.1 编写单元测试
