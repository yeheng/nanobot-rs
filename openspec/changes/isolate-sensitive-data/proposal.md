# Change: Isolate Sensitive Data from LLM Access

## Why

当前所有敏感信息（API keys, tokens, passwords）直接存储在 `~/.nanobot/config.yaml` 中，LLM 通过 history/memory 机制可以读取这些文件。这导致：
1. Token 和密码可能泄露给 LLM 服务提供商
2. 无法实现最小权限原则
3. 违反安全最佳实践

## What Changes

- 新增 vault 模块用于加密存储敏感信息
- config.yaml 中的敏感字段改为引用格式 (`ref:token-name`)
- 运行时由用户解锁 vault，解密后的凭据仅保存在内存中
- LLM 和 history 只能看到引用字符串，无法获取真实凭据

**非破坏性变更**：现有 config 格式保持兼容，用户可以选择性迁移到 vault。

## Impact

- **Affected specs**:
  - `config` - 配置加载需要解析 ref: 前缀
  - `auth` - auth 命令需要将 token 存入 vault
  - `memory` - memory 模块需要确保不缓存已解析的敏感值
  - `vault` - 新增能力

- **Affected code**:
  - `nanobot-core/src/config/loader.rs` - 添加 ref 解析
  - `nanobot-core/src/vault/` - 新增模块
  - `nanobot-cli/src/commands/auth.rs` - 修改存储逻辑
  - `nanobot-cli/src/commands/vault.rs` - 新增命令
