# Capability: MCP Protocol for Email

## ADDED Requirements

### Requirement: MCP Tools
系统 SHALL 实现 MCP 协议定义的工具接口。

#### Scenario: index_emails tool
- **WHEN** MCP 客户端调用 `index_emails` tool 并提供邮件列表
- **THEN** 索引所有邮件并返回成功数量和失败列表

#### Scenario: search_emails tool
- **WHEN** MCP 客户端调用 `search_emails` tool 并提供查询参数
- **THEN** 返回匹配的邮件列表（包含摘要信息）

#### Scenario: get_email tool
- **WHEN** MCP 客户端调用 `get_email` tool 并提供邮件 ID
- **THEN** 返回单封邮件的完整内容

#### Scenario: delete_email tool
- **WHEN** MCP 客户端调用 `delete_email` tool 并提供邮件 ID
- **THEN** 从索引中删除该邮件并返回确认

#### Scenario: get_index_stats tool
- **WHEN** MCP 客户端调用 `get_index_stats` tool
- **THEN** 返回索引统计信息（邮件数量、存储大小等）

### Requirement: MCP Resources
系统 SHALL 实现 MCP 协议定义的资源接口。

#### Scenario: Email resource URI
- **WHEN** MCP 客户端请求 `email://{id}` 资源
- **THEN** 返回对应邮件的内容

### Requirement: MCP Server
系统 SHALL 作为标准 MCP 服务器运行。

#### Scenario: STDIO transport
- **WHEN** 通过 stdin 接收 MCP 请求
- **THEN** 通过 stdout 发送 MCP 响应

#### Scenario: Server initialization
- **WHEN** MCP 客户端发送 `initialize` 请求
- **THEN** 返回服务器能力和工具列表
