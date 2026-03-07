# Vault Isolation - Task List

## 状态追踪

| Task | What | Why | Status |
|------|------|-----|--------|
| T1 | VaultStore核心实现 | 敏感数据独立存储 | ⬜ Pending |
| T2 | Placeholder扫描器 | 检测`{{vault:*}}` | ⬜ Pending |
| T3 | VaultInjector | 运行时注入 | ⬜ Pending |
| T4 | AgentLoop集成 | 注入到数据流 | ⬜ Pending |
| T5 | 历史保存保护 | 保存placeholder而非明文 | ⬜ Pending |
| T6 | 日志过滤 | 防止敏感数据进入日志 | ⬜ Pending |
| T7 | CLI命令 | vault set/get/list | ⬜ Pending |
| T8 | 加密存储 | 可选的加密支持 | ⬜ Pending |
| T9 | 单元测试 | 验证核心功能 | ⬜ Pending |
| T10 | 集成测试 | 端到端验证 | ⬜ Pending |

---

## T1: VaultStore核心实现

### What
实现敏感数据的独立存储组件，与memory/history完全隔离。

### Why
敏感数据需要独立的安全存储，不应与普通memory混在一起。

### Where
- 新建: `nanobot-rs/nanobot-core/src/vault/mod.rs`
- 新建: `nanobot-rs/nanobot-core/src/vault/store.rs`

### How

```rust
// src/vault/store.rs

use std::collections::HashMap;
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 敏感条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub key: String,
    pub value: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

/// 元数据 (不包含value，用于list)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMetadata {
    pub key: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

/// Vault存储
pub struct VaultStore {
    path: PathBuf,
    entries: HashMap<String, VaultEntry>,
}

impl VaultStore {
    pub fn new(path: PathBuf) -> Result<Self, VaultError> {
        // 加载现有数据
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|e| e.value.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str, description: Option<&str>) -> Result<(), VaultError> {
        // 添加或更新条目
        // 持久化到磁盘
    }

    pub fn list_keys(&self) -> Vec<VaultMetadata> {
        // 返回所有key的元数据
    }

    pub fn delete(&mut self, key: &str) -> Result<(), VaultError> {
        // 删除条目
    }
}
```

### Test Case & Acceptance Criteria

```
AC1: 能够set和get敏感数据
AC2: 数据持久化到独立文件
AC3: list_keys不返回value
AC4: 删除后无法get
```

---

## T2: Placeholder扫描器

### What
实现正则扫描器，检测文本中的`{{vault:key}}`占位符。

### Why
需要在用户输入和历史消息中识别需要注入的位置。

### Where
- 新建: `nanobot-rs/nanobot-core/src/vault/scanner.rs`

### How

```rust
// src/vault/scanner.rs

use regex::Regex;

/// Placeholder模式
pub const PLACEHOLDER_PATTERN: &str = r"\{\{vault:([a-zA-Z0-9_]+)\}\}";

/// 扫描结果
#[derive(Debug, Clone)]
pub struct Placeholder {
    pub key: String,
    pub full_match: String,
    pub start: usize,
    pub end: usize,
}

/// 扫描文本中的placeholder
pub fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    let re = Regex::new(PLACEHOLDER_PATTERN).unwrap();
    re.captures_iter(text)
        .filter_map(|cap| {
            let full = cap.get(0)?;
            let key = cap.get(1)?;
            Some(Placeholder {
                key: key.as_str().to_string(),
                full_match: full.as_str().to_string(),
                start: full.start(),
                end: full.end(),
            })
        })
        .collect()
}

/// 替换文本中的placeholder
pub fn replace_placeholders(text: &str, replacements: &HashMap<String, String>) -> String {
    let re = Regex::new(PLACEHOLDER_PATTERN).unwrap();
    re.replace_all(text, |cap: &regex::Captures| {
        let key = &cap[1];
        replacements.get(key)
            .map(|s| s.as_str())
            .unwrap_or_else(|| cap.get(0).unwrap().as_str())
    }).to_string()
}
```

### Test Case & Acceptance Criteria

```
AC1: 正确识别单个placeholder
AC2: 正确识别多个placeholder
AC3: 不匹配无效格式
AC4: 正确替换placeholder
AC5: 未知的key保持原样
```

---

## T3: VaultInjector

### What
实现运行时注入器，在发送给LLM前替换所有placeholder。

### Why
确保敏感数据只在最终时刻注入，不在任何存储中出现。

### Where
- 新建: `nanobot-rs/nanobot-core/src/vault/injector.rs`

### How

```rust
// src/vault/injector.rs

use std::sync::Arc;
use crate::providers::ChatMessage;
use super::{VaultStore, scan_placeholders, replace_placeholders};

/// 注入报告
#[derive(Debug, Default)]
pub struct InjectionReport {
    pub replaced_count: usize,
    pub keys_used: Vec<String>,
    pub missing_keys: Vec<String>,
}

/// Vault注入器
pub struct VaultInjector {
    store: Arc<VaultStore>,
}

impl VaultInjector {
    pub fn new(store: Arc<VaultStore>) -> Self {
        Self { store }
    }

    /// 注入所有消息中的placeholder
    pub fn inject(&self, messages: &mut [ChatMessage]) -> InjectionReport {
        let mut report = InjectionReport::default();
        let mut all_keys = std::collections::HashSet::new();

        // 收集所有需要的key
        for msg in messages.iter() {
            for placeholder in scan_placeholders(&msg.content) {
                all_keys.insert(placeholder.key);
            }
        }

        // 构建替换映射
        let mut replacements = HashMap::new();
        for key in &all_keys {
            if let Some(value) = self.store.get(key) {
                replacements.insert(key.clone(), value.to_string());
                report.keys_used.push(key.clone());
            } else {
                report.missing_keys.push(key.clone());
            }
        }

        // 执行替换
        for msg in messages.iter_mut() {
            let original = &msg.content;
            let replaced = replace_placeholders(original, &replacements);
            if replaced != *original {
                report.replaced_count += 1;
                msg.content = replaced;
            }
        }

        report
    }
}
```

### Test Case & Acceptance Criteria

```
AC1: 正确注入单个消息
AC2: 正确注入多个消息
AC3: 报告缺失的key
AC4: 不修改无placeholder的消息
AC5: 保留原始消息的placeholder（在历史保存时）
```

---

## T4: AgentLoop集成

### What
将VaultInjector集成到AgentLoop的处理流程中。

### Why
在数据发送给LLM前的最后时刻注入敏感数据。

### Where
- 修改: `nanobot-rs/nanobot-core/src/agent/loop_.rs`

### How

```rust
// 在 AgentLoop 结构体中添加
pub struct AgentLoop {
    // ... 现有字段 ...
    vault_injector: Option<Arc<VaultInjector>>,
}

// 在 process_direct_with_callback 中修改
pub async fn process_direct_with_callback(...) -> Result<AgentResponse, AgentError> {
    // ... 现有代码 ...

    // ── 7. Assemble prompt (pure, synchronous) ─────────────────
    let mut messages = Self::assemble_prompt(
        processed.messages,
        content,  // 注意：这里用原始content，包含placeholder
        &system_prompts,
        summary.as_deref(),
    );

    // ── 7.5. Inject vault secrets (NEW) ────────────────────────
    let vault_keys_used = if let Some(ref injector) = self.vault_injector {
        let report = injector.inject(&mut messages);
        if !report.missing_keys.is_empty() {
            warn!("[Vault] Missing keys: {:?}", report.missing_keys);
        }
        report.keys_used
    } else {
        vec![]
    };

    // ── 8. Run agent loop ─────────────────────────────────────
    let result = self.run_agent_loop(messages, effective_cb).await?;

    // ... 现有代码 ...
}
```

### Test Case & Acceptance Criteria

```
AC1: 包含placeholder的消息正确注入
AC2: 历史保存的是原始placeholder
AC3: LLM收到的是注入后的明文
AC4: 无vault配置时正常工作
```

---

## T5: 历史保存保护

### What
确保历史保存的是原始placeholder，而不是注入后的明文。

### Why
敏感数据不应出现在SQLite历史记录中。

### Where
- 修改: `nanobot-rs/nanobot-core/src/agent/loop_.rs` (process_direct_with_callback)

### How

关键点：在 `process_direct_with_callback` 中，保存用户消息时使用的是原始 `content` 参数，而不是注入后的版本。

```rust
// ── 3. Save user message (direct, Option-aware) ────────────────
// content 是原始输入，包含 {{vault:*}} 而非明文
if let Some(ref sm) = self.session_manager {
    if let Err(e) = sm.append_by_key(session_key, "user", content, None).await {
        warn!("Failed to persist user message: {}", e);
    }
}
```

对于assistant消息，需要确保不包含敏感数据：

```rust
// ── 10. Save assistant message ───────────────────────────────────
if let Some(ref sm) = self.session_manager {
    // 清理assistant消息中的敏感数据
    let safe_content = if !vault_keys_used.is_empty() {
        redact_vault_values(&result.content, &vault_keys_used, self.vault_injector.as_ref())
    } else {
        result.content.clone()
    };

    sm.append_by_key(session_key, "assistant", &safe_content, Some(result.tools_used.clone())).await?;
}
```

### Test Case & Acceptance Criteria

```
AC1: 用户消息保存placeholder
AC2: assistant消息不含敏感数据
AC3: SQLite中搜索敏感数据无结果
```

---

## T6: 日志过滤

### What
在日志输出前过滤敏感数据。

### Why
防止敏感数据出现在日志文件中。

### Where
- 新建: `nanobot-rs/nanobot-core/src/vault/redaction.rs`
- 修改: `nanobot-rs/nanobot-core/src/agent/loop_.rs`

### How

```rust
// src/vault/redaction.rs

/// 替换敏感值为 [REDACTED]
pub fn redact_secrets(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            result = result.replace(secret, "[REDACTED]");
        }
    }
    result
}

// 在 log_llm_response 中使用
fn log_llm_response(response: &ChatResponse, iteration: u32, vault_values: &[String]) {
    let content = response.content.as_ref().map(|c| {
        redact_secrets(c, vault_values)
    });
    // ...
}
```

### Test Case & Acceptance Criteria

```
AC1: 敏感数据被替换为[REDACTED]
AC2: 非敏感数据保持不变
AC3: 日志文件中无明文敏感数据
```

---

## T7: CLI命令

### What
添加vault管理的CLI命令。

### Why
用户需要方便地管理敏感数据。

### Where
- 修改: `nanobot-rs/nanobot-cli/src/main.rs`
- 新建: `nanobot-rs/nanobot-cli/src/commands/vault.rs`

### How

```bash
# 设置敏感数据
nanobot vault set db_password
> Enter value: ********
> Description (optional): Database password

# 获取敏感数据 (显示是否存在)
nanobot vault get db_password
> Key: db_password
> Description: Database password
> Created: 2026-03-07 10:00:00
> Value: ******** (hidden, use --show to reveal)

# 列出所有key
nanobot vault list
> db_password - Database password
> api_key - API key for service X

# 删除
nanobot vault delete db_password
> Deleted: db_password
```

### Test Case & Acceptance Criteria

```
AC1: set命令正确保存
AC2: get命令显示元数据
AC3: list命令列出所有key
AC4: delete命令正确删除
```

---

## T8: 加密存储 (可选)

### What
支持加密存储敏感数据。

### Why
增强安全性，防止文件泄露。

### Where
- 修改: `nanobot-rs/nanobot-core/src/vault/store.rs`

### How

使用系统keychain或用户密码加密：

```rust
pub enum EncryptionMethod {
    None,
    Keychain,  // 使用系统keychain
    Password,  // 使用用户密码
}

impl VaultStore {
    pub fn save(&self) -> Result<(), VaultError> {
        let json = serde_json::to_string(&self.entries)?;
        let data = match self.encryption {
            EncryptionMethod::None => json.into_bytes(),
            EncryptionMethod::Keychain => {
                // 使用keychain API加密
                encrypt_with_keychain(&json)?
            }
            EncryptionMethod::Password => {
                // 使用密码加密
                encrypt_with_password(&json, &self.password)?
            }
        };
        std::fs::write(&self.path, data)?;
        Ok(())
    }
}
```

### Test Case & Acceptance Criteria

```
AC1: 无加密模式正常工作
AC2: keychain加密正确工作
AC3: 密码加密正确工作
AC4: 加密文件无法直接读取
```

---

## T9: 单元测试

### What
为核心组件编写单元测试。

### Why
确保功能正确性。

### Where
- 新建: `nanobot-rs/nanobot-core/src/vault/store_tests.rs`
- 新建: `nanobot-rs/nanobot-core/src/vault/scanner_tests.rs`
- 新建: `nanobot-rs/nanobot-core/src/vault/injector_tests.rs`

### How

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_single_placeholder() {
        let text = "使用 {{vault:db_password}} 连接数据库";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].key, "db_password");
    }

    #[test]
    fn test_inject_replaces_placeholders() {
        let mut store = VaultStore::new_in_memory();
        store.set("api_key", "sk-12345", None).unwrap();

        let injector = VaultInjector::new(Arc::new(store));
        let mut messages = vec![ChatMessage::user("使用 {{vault:api_key}}")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.replaced_count, 1);
        assert_eq!(messages[0].content, "使用 sk-12345");
    }

    #[test]
    fn test_history_preserves_placeholders() {
        // 验证保存的是placeholder而非明文
    }
}
```

### Test Case & Acceptance Criteria

```
AC1: 所有测试通过
AC2: 覆盖率 > 80%
AC3: 边界情况覆盖
```

---

## T10: 集成测试

### What
端到端测试vault功能。

### Why
验证整体功能正确性。

### Where
- 新建: `nanobot-rs/tests/vault_integration_tests.rs`

### How

```rust
#[tokio::test]
async fn test_e2e_vault_injection() {
    // 1. 设置vault
    let vault = VaultStore::new_in_memory();
    vault.set("test_key", "secret_value", None).unwrap();

    // 2. 创建agent with vault
    let agent = create_test_agent_with_vault(vault);

    // 3. 发送包含placeholder的消息
    let response = agent.process_direct("使用 {{vault:test_key}}", &session_key).await.unwrap();

    // 4. 验证LLM收到了正确的值
    assert!(response.content.contains("secret_value"));

    // 5. 验证历史保存的是placeholder
    let history = agent.get_history(&session_key).await;
    assert!(history[0].content.contains("{{vault:test_key}}"));
    assert!(!history[0].content.contains("secret_value"));
}
```

### Test Case & Acceptance Criteria

```
AC1: 端到端流程正确
AC2: 敏感数据不在历史中出现
AC3: 敏感数据不在memory中出现
AC4: 敏感数据不在日志中出现
```

---

## 依赖关系

```
T1 (VaultStore) ──┬──> T3 (Injector)
                  │
T2 (Scanner) ─────┘
                    │
                    v
T4 (AgentLoop) ──> T5 (History) ──> T6 (Logging)
                    │
                    v
T7 (CLI) ──> T8 (Encryption)
                    │
                    v
T9 (Unit Tests) ──> T10 (Integration Tests)
```

## 优先级

1. **P0 (必须)**: T1, T2, T3, T4, T5, T9
2. **P1 (重要)**: T6, T7, T10
3. **P2 (可选)**: T8
