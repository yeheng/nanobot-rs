# Capability: Email MCP Index

## ADDED Requirements

### Requirement: Email Document Schema
系统 SHALL 定义标准化的邮件文档结构用于索引。

#### Scenario: Email with all fields
- **WHEN** 外部系统提供完整邮件数据
- **THEN** 索引包含 id、subject、from、to、body、date、labels

#### Scenario: Email with minimal fields
- **WHEN** 外部系统只提供必要字段（id、body）
- **THEN** 索引成功，其他字段使用默认值

### Requirement: Email Indexing
系统 SHALL 能够将外部提供的邮件内容索引到 Tantivy 搜索引擎中。

#### Scenario: Batch index
- **WHEN** MCP 客户端调用 `index_emails` tool 提供多封邮件
- **THEN** 所有邮件被索引，返回成功数量和失败列表

#### Scenario: Incremental index via idempotency
- **WHEN** 索引已存在相同 id 的邮件
- **THEN** 更新现有文档，不创建重复

### Requirement: Email Search
系统 SHALL 提供邮件搜索功能，支持全文检索和过滤。

#### Scenario: Full-text search
- **WHEN** 用户提供搜索关键词
- **THEN** 返回匹配的邮件列表（按相关性排序）

#### Scenario: Filter by date range
- **WHEN** 用户提供日期范围
- **THEN** 只返回该范围内的邮件

#### Scenario: Filter by sender/recipient
- **WHEN** 用户提供发件人或收件人地址
- **THEN** 只返回匹配的邮件

#### Scenario: Filter by labels
- **WHEN** 用户提供标签列表
- **THEN** 只返回包含任一标签的邮件

### Requirement: Email Retrieval
系统 SHALL 能够根据 ID 检索单封邮件的完整内容。

#### Scenario: Get email by ID
- **WHEN** 用户提供邮件 ID
- **THEN** 返回邮件的完整内容（所有字段）

#### Scenario: Email not found
- **WHEN** 提供的 ID 不存在于索引中
- **THEN** 返回错误信息

### Requirement: Index Management
系统 SHALL 提供索引管理功能。

#### Scenario: Index statistics
- **WHEN** 用户查询索引状态
- **THEN** 返回索引中的邮件数量、存储大小等信息

#### Scenario: Clear index
- **WHEN** 用户请求清空索引
- **THEN** 删除所有索引数据
