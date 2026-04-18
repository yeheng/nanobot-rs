# Vault 使用指南

Vault 是 Gasket 的敏感数据隔离模块，用于安全地存储和管理敏感信息（如 API 密钥、密码、数据库连接字符串等）。

## 目录

- [概述](#概述)
- [设计原则](#设计原则)
- [快速开始](#快速开始)
- [CLI 命令参考](#cli-命令参考)
- [占位符语法](#占位符语法)
- [安全最佳实践](#安全最佳实践)
- [常见用例](#常见用例)

## 概述

Vault 的核心功能是将敏感数据与 LLM 可访问的存储完全隔离，确保：

- 敏感数据永远不会持久化到 LLM 的记忆或历史记录中
- 只有在发送给 LLM 的最后一刻才会注入敏感数据
- 支持在消息中使用占位符语法 `{{vault:key}}`

### 存储位置

```
~/.gasket/vault/secrets.json
```

### 加密算法

Vault 使用 **XChaCha20-Poly1305** 进行加密（默认）：

- **算法**: XChaCha20-Poly1305（AEAD 认证加密）
- **密钥派生**: Argon2id（内存-hard 密钥派生函数）
- **密码环境变量**: `GASKET_MASTER_PASSWORD`

> **注意**: XChaCha20-Poly1305 是唯一支持的加密算法，提供更强的 nonce 随机性和现代密码学安全性。

## 设计原则

### 1. 数据结构隔离

VaultStore 完全独立于 memory（记忆）和 history（历史）存储系统，存储在单独的 JSON 文件中。

### 2. 运行时注入

通过 `VaultInjector` 拦截器，在消息发送给 LLM 之前扫描并替换占位符：

```rust
// 消息中的占位符
"Connect to database with {{vault:db_password}}"

// 运行时被替换为
"Connect to database with actual_password_value"
```

### 3. 零信任设计

敏感数据永远不会出现在：
- LLM 的持久化存储中
- 日志文件中（支持日志脱敏）
- 会话历史中

## 快速开始

### 1. 设置密钥

```bash
# 交互式设置（会提示输入值）
gasket vault set openai_api_key

# 直接设置值和描述
gasket vault set db_password --value "my_secret_password" --description "Database password"
```

### 2. 查看密钥列表

```bash
# 列出所有密钥（值被隐藏）
gasket vault list
```

输出示例：

```
Vault Entries

Key                 Description                               Created
────────────────────────────────────────────────────────────────────────────────
openai_api_key      OpenAI API key for chat completions       2024-01-15 10:30
db_password         Database password                         2024-01-15 11:00

Total: 2 entries

Tip: Use {{vault:key}} in your messages to inject secrets at runtime.
```

### 3. 在对话中使用

在与 Agent 对话时，使用占位符语法：

```
请使用 API 密钥 {{vault:openai_api_key}} 来调用 OpenAI API
```

Agent 收到的消息中，`{{vault:openai_api_key}}` 会被自动替换为实际的密钥值。

## CLI 命令参考

### `gasket vault list`

列出所有 vault 条目（值被隐藏）。

```bash
gasket vault list
```

### `gasket vault set`

设置一个 vault 条目。

```bash
# 交互式设置
gasket vault set <key>

# 直接设置
gasket vault set <key> --value <value> --description <description>

# 简写
gasket vault set <key> -v <value> -d <description>
```

**参数：**
- `key` - 密钥名称（只能包含字母、数字和下划线）
- `--value, -v` - 密钥值（可选，不提供则会交互式提示）
- `--description, -d` - 描述信息（可选）

**示例：**

```bash
# 交互式设置（密码输入会被隐藏）
gasket vault set api_key

# 直接设置
gasket vault set aws_access_key -v "AKIAIOSFODNN7EXAMPLE" -d "AWS Access Key"

# 更新现有条目（会保留创建时间）
gasket vault set api_key -v "new_value"
```

### `gasket vault get`

获取密钥值（直接输出到标准输出）。

```bash
gasket vault get <key>
```

**注意：** 此命令会直接输出密钥值，请谨慎使用。

**用例：**

```bash
# 在脚本中使用
export API_KEY=$(gasket vault get openai_api_key)

# 复制到剪贴板（macOS）
gasket vault get api_key | pbcopy
```

### `gasket vault show`

显示密钥的详细信息。

```bash
gasket vault show <key>

# 显示密钥值
gasket vault show <key> --show-value
```

**参数：**
- `--show-value` - 显示密钥值（默认不显示）

**输出示例：**

```
openai_api_key

  Description: OpenAI API key
  Created:     2024-01-15 10:30 UTC
  Last used:   2024-01-16 14:22 UTC

  Use --show-value to display the secret value.

  Usage: {{vault:openai_api_key}}
```

### `gasket vault delete`

删除一个密钥。

```bash
gasket vault delete <key>

# 跳过确认提示
gasket vault delete <key> --force
```

**参数：**
- `--force, -f` - 跳过确认提示

### `gasket vault import`

从 JSON 文件导入密钥。

```bash
gasket vault import <file>

# 合并模式（不覆盖已存在的密钥）
gasket vault import <file> --merge
```

**参数：**
- `--merge, -m` - 合并模式，不覆盖已存在的密钥

**JSON 文件格式：**

```json
{
  "api_key": {
    "key": "api_key",
    "value": "sk-xxxx",
    "description": "API Key",
    "created_at": "2024-01-15T10:30:00Z",
    "last_used": null
  }
}
```

### `gasket vault export`

导出所有密钥到 JSON 文件。

```bash
gasket vault export <file>
```

**示例：**

```bash
gasket vault export ~/backup/vault_backup.json
```

**注意：** 导出的文件包含所有密钥值，请妥善保管！

## 占位符语法

在消息中使用 `{{vault:key_name}}` 格式的占位符：

### 基本用法

```
使用 {{vault:api_key}} 来访问 API
```

### 多个占位符

```
连接数据库：
- 主机: db.example.com
- 用户: admin
- 密码: {{vault:db_password}}
```

### 支持的位置

占位符可以出现在消息的任何位置：

```
系统提示: 你是一个助手，使用 {{vault:system_token}} 进行认证。
用户消息: 请帮我调用 {{vault:service_name}} 服务。
```

### 占位符命名规则

密钥名称必须：
- 非空
- 只包含字母（a-z, A-Z）、数字（0-9）和下划线（_）
- 以字母或下划线开头

**有效示例：**
- `api_key`
- `db_password_v2`
- `AWS_SECRET_KEY`
- `token123`

**无效示例：**
- `api-key`（包含连字符）
- `db.password`（包含点）
- `123key`（以数字开头，虽然技术上允许但不推荐）

## 安全最佳实践

### 1. 文件权限

确保 vault 文件权限正确：

```bash
chmod 600 ~/.gasket/vault/secrets.json
```

### 2. 环境变量加密

设置 vault 密码环境变量以启用加密：

```bash
export GASKET_VAULT_PASSWORD="your-strong-password"
```

如果没有设置此环境变量，vault 会提示输入密码。未加密的 vault 文件仍然受文件系统权限保护，但强烈建议启用加密。

### 2. 不要在代码中硬编码

避免：

```rust
// ❌ 不要这样做
let api_key = "sk-xxxx";
```

推荐：

```rust
// ✅ 使用 vault
let message = "Use {{vault:api_key}} to authenticate";
```

### 3. 定期轮换密钥

```bash
# 更新密钥值
gasket vault set api_key -v "new_secret_value"
```

### 4. 备份和恢复

```bash
# 备份
gasket vault export ~/secure_backup/vault_$(date +%Y%m%d).json

# 恢复
gasket vault import ~/secure_backup/vault_20240115.json
```

### 5. 审计使用情况

```bash
# 查看密钥最后使用时间
gasket vault show api_key
```

### 6. 最小权限原则

只为需要的密钥设置访问权限，不使用的密钥应该删除：

```bash
gasket vault delete unused_key
```

## 常见用例

### API 密钥管理

```bash
# 设置多个 API 密钥
gasket vault set openai_api_key -d "OpenAI API Key"
gasket vault set anthropic_api_key -d "Anthropic API Key"
gasket vault set github_token -d "GitHub Personal Access Token"
```

在对话中：

```
请使用 {{vault:openai_api_key}} 调用 GPT-4 API 来帮我分析这段代码。
```

### 数据库连接

```bash
# 存储数据库凭据
gasket vault set db_host -v "db.example.com" -d "Database host"
gasket vault set db_user -v "admin" -d "Database user"
gasket vault set db_password -v "secret123" -d "Database password"
```

在对话中：

```
帮我写一个连接到 {{vault:db_host}} 的数据库查询，用户名是 admin，密码是 {{vault:db_password}}
```

### AWS 凭证

```bash
gasket vault set aws_access_key -v "AKIAIOSFODNN7EXAMPLE" -d "AWS Access Key"
gasket vault set aws_secret_key -v "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY" -d "AWS Secret Key"
gasket vault set aws_region -v "us-east-1" -d "AWS Region"
```

### CI/CD 集成

在 CI/CD 管道中使用：

```yaml
# .github/workflows/deploy.yml
- name: Get API Key
  run: |
    export API_KEY=$(gasket vault get production_api_key)
    ./deploy.sh
```

## 故障排除

### 找不到密钥

如果看到 `Key not found` 错误：

1. 检查密钥名称拼写
2. 使用 `gasket vault list` 查看所有可用密钥
3. 确保密钥名称只包含字母、数字和下划线

### 占位符未被替换

如果消息中的占位符没有被替换：

1. 确认格式正确：`{{vault:key}}`（注意是双层大括号）
2. 检查密钥是否存在
3. 检查密钥名称是否正确

### 权限错误

如果遇到文件权限错误：

```bash
# 修复权限
chmod 700 ~/.gasket/vault
chmod 600 ~/.gasket/vault/secrets.json
```

## 相关文档

- [Architecture Overview](./architecture.md)
- [Data Structures](./data-structures.md)
- [Modules](./modules.md)
