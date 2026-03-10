# Design: Email MCP Index Service

## Context
- 需要一个独立的 MCP 服务器来处理邮件索引和查询
- 使用 Tantivy 作为搜索引擎（与现有 nanobot-tantivy 一致）
- **不需要 IMAP**：外部系统负责获取邮件，通过 MCP tool 传入 email 内容
- 需要与 nanobot-core 通过 MCP 协议集成

## Goals / Non-Goals
- Goals:
  - 提供完整的邮件索引和查询功能
  - 实现标准 MCP 协议
  - 支持增量索引（通过 email_id 去重）
  - 提供 CLI 管理工具
- Non-Goals:
  - 邮件获取（IMAP/POP3）- 由外部系统负责
  - 邮件发送功能
  - 多账户管理（留给未来）
  - Web UI

## Decisions
- Decision: 使用独立的 crate `nanobot-email-mcp`
  - 为什么：保持模块化，可独立部署和测试
- Decision: 外部系统负责邮件获取
  - 为什么：职责分离，MCP 专注索引和查询
- Decision: Tantivy 索引独立存储
  - 为什么：与 nanobot-core 的记忆系统分离
- Decision: Email schema 包含主题、发件人、收件人、正文、日期、标签

## Alternatives Considered
- 方案 1: 集成 IMAP 到 MCP 服务器
  - 拒绝原因：职责过重，外部系统可能已有邮件获取逻辑
- 方案 2: 集成到 nanobot-core 内部
  - 拒绝原因：MCP 服务器需要独立部署

## Risks / Trade-offs
- 依赖外部系统提供邮件内容 → 定义清晰的 email schema
- 大量邮件的索引性能 → 实现批处理索引
- 敏感信息安全 → 考虑加密存储索引

## Email Schema Design
```rust
pub struct EmailDocument {
    pub id: String,           // 唯一标识（外部系统提供，如 Message-ID）
    pub subject: String,      // 邮件主题
    pub from: String,         // 发件人地址
    pub to: Vec<String>,      // 收件人地址列表
    pub cc: Vec<String>,      // 抄送地址列表
    pub body_text: String,    // 纯文本正文
    pub body_html: Option<String>, // HTML 正文（可选）
    pub date: chrono::DateTime<chrono::Utc>, // 邮件日期
    pub labels: Vec<String>,  // 标签/分类
    pub indexed_at: chrono::DateTime<chrono::Utc>, // 索引时间
}
```

## Migration Plan
- 无需迁移，新功能

## Open Questions
- 是否需要支持附件索引？（未来扩展）
- 是否需要支持邮件线程（thread）？（未来扩展）
