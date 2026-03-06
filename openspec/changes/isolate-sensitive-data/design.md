## Context

当前敏感信息直接存储在 `~/.nanobot/config.yaml` 中，LLM 可以通过 history/memory 读取这些文件。需要将敏感信息隔离到加密的 vault 中。

## Goals / Non-Goals

**Goals:**
- 敏感信息加密存储，LLM 无法直接读取
- 运行时按需解锁，进程内存中持有解密值
- 向后兼容现有 config 格式
- 支持 macOS Keychain 集成（可选）

**Non-Goals:**
- 不支持多用户共享 vault
- 不支持远程 vault 服务
- 不支持硬件安全密钥（YubiKey 等）

## Decisions

### Decision 1: File-based Encrypted Vault
**What**: 使用 AES-GCM-256 加密文件存储凭据，主密码通过 PBKDF2 派生密钥。
**Why**:
- 简单，不依赖外部服务
- AES-GCM 提供认证加密，防篡改
- PBKDF2 是成熟的密钥派生函数

### Decision 2: ref: Prefix Resolution
**What**: config.yaml 中使用 `ref:token-name` 引用 vault 中的凭据。
**Why**:
- 明确标识哪些字段是敏感引用
- 解析逻辑简单，不会误解析
- LLM 看到引用字符串也无法推断真实值

### Decision 3: Session-based Unlock
**What**: 运行时解锁 vault，解密数据保存在内存中，进程退出时自动清除。
**Why**:
- 用户只需解锁一次 per session
- 内存数据比文件更安全（假设无 swap）
- 符合 "unlock once, use many times" 模式

### Decision 4: mlock for Memory Protection
**What**: 使用 `mlock()` 防止解密数据被 swap 到磁盘。
**Why**:
- 防止冷启动攻击
- 防止 swap 文件分析
- Rust 有成熟的 `mlock` crate 可用

## Alternatives Considered

### Alternative 1: Environment Variables
**Rejected**:
- env vars 仍然可以被进程读取
- 无法防止 LLM 通过 history 获取
- 生命周期管理复杂

### Alternative 2: External Vault Service (HashiCorp Vault)
**Rejected**:
- 太重，不符合 KISS 原则
- 增加运维复杂度
- 对本地应用过度设计

### Alternative 3: Encrypt Entire Config
**Rejected**:
- 每次读取都需要解密整个文件
- 无法部分更新配置
- 用户体验差

## Migration Plan

### Phase 1: Core Implementation
1. 实现 vault 模块（加密/解密）
2. 实现 config loader ref 解析
3. 实现 CLI 命令

### Phase 2: Migration
1. 更新 auth 命令使用 vault
2. 提供迁移脚本帮助老用户
3. 文档更新

### Phase 3: Hardening
1. 添加 mlock 保护
2. 审计日志输出，确保无泄露
3. 可选 keychain 集成

## Security Considerations

1. **Master Password**: 必须足够强，建议 12+ 字符
2. **Memory Zeroing**: 所有敏感缓冲区和变量在 drop 时清零
3. **No Logging**: 敏感值绝对不能进入 tracing 日志
4. **Swap Prevention**: mlock 防止交换到磁盘

## Open Questions

1. 是否支持 biometric unlock（Touch ID）？
2. 是否支持多 vault 配置（个人/工作分离）？
