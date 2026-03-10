# Change: 添加邮件 MCP 索引服务

## Why
提供一个独立的 MCP (Model Context Protocol) 服务器，用于对邮件进行索引和查询，使 AI agent 能够直接通过 MCP 协议访问和搜索邮件数据。使用 Tantivy 作为搜索引擎，提供高效的全文检索能力。

## What Changes
- 创建新的子项目 `nanobot-email-mcp`
- 实现邮件索引功能（使用 Tantivy）
- 实现邮件查询功能（支持全文搜索、过滤）
- 实现 MCP 服务器协议（tools、resources）
- 支持 IMAP 邮件抓取和索引
- 提供 CLI 工具管理索引

## Impact
- 新增子项目：`nanobot-email-mcp`
- 新增 MCP 能力：邮件索引和查询
- 可被 nanobot-core 或其他 MCP 客户端调用
- 需要配置 IMAP 服务器信息

## Scope
- **In Scope**:
  - 邮件抓取（IMAP）
  - 邮件索引（Tantivy）
  - MCP 服务器实现
  - 基础 CLI 管理命令
- **Out of Scope**:
  - 邮件发送功能
  - 邮件客户端 UI
  - 多账户管理（留作未来扩展）
