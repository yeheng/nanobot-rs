# Vault Module Specification

## 模块结构

```
nanobot-core/src/vault/
├── mod.rs           # 模块导出
├── store.rs         # VaultStore - 敏感数据存储
├── scanner.rs       # Placeholder扫描器
├── injector.rs      # VaultInjector - 运行时注入
├── redaction.rs     # 日志过滤工具
└── error.rs         # 错误类型
```

---

## 1. Error Types

```rust
// src/vault/error.rs

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("Vault entry not found: {0}")]
    NotFound(String),

    #[error("Vault entry already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid key name: {0}")]
    InvalidKey(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Encryption error: {0}")]
    Encryption(String),
}
```

---

## 2. VaultStore

```rust
// src/vault/store.rs

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::VaultError;

/// 敏感条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    /// 唯一标识符 (用于placeholder: {{vault:key}})
    pub key: String,
    /// 敏感值
    pub value: String,
    /// 描述
    pub description: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后使用时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
}

/// 元数据 (不包含value)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMetadata {
    pub key: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

impl From<&VaultEntry> for VaultMetadata {
    fn from(entry: &VaultEntry) -> Self {
        Self {
            key: entry.key.clone(),
            description: entry.description.clone(),
            created_at: entry.created_at,
            last_used: entry.last_used,
        }
    }
}

/// Vault存储
///
/// 存储位置: ~/.nanobot/vault/secrets.json
/// 与SQLite和Markdown完全隔离
pub struct VaultStore {
    /// 存储文件路径
    path: PathBuf,
    /// 内存中的条目
    entries: RwLock<HashMap<String, VaultEntry>>,
}

impl VaultStore {
    /// 默认存储路径
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join(".nanobot")
            .join("vault")
            .join("secrets.json")
    }

    /// 创建新的VaultStore
    pub fn new() -> Result<Self, VaultError> {
        Self::with_path(Self::default_path())
    }

    /// 使用指定路径创建
    pub fn with_path(path: PathBuf) -> Result<Self, VaultError> {
        // 确保目录存在
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // 加载现有数据
        let entries = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            if content.trim().is_empty() {
                HashMap::new()
            } else {
                let loaded: HashMap<String, VaultEntry> = serde_json::from_str(&content)?;
                info!("[Vault] Loaded {} entries from {:?}", loaded.len(), path);
                loaded
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            entries: RwLock::new(entries),
        })
    }

    /// 创建内存版本 (用于测试)
    pub fn new_in_memory() -> Self {
        Self {
            path: PathBuf::from(":memory:"),
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// 获取敏感值
    pub fn get(&self, key: &str) -> Option<String> {
        let entries = self.entries.read().unwrap();
        entries.get(key).map(|e| {
            // 更新 last_used (在单独的写操作中)
            drop(entries);
            let mut entries = self.entries.write().unwrap();
            if let Some(entry) = entries.get_mut(key) {
                entry.last_used = Some(Utc::now());
            }
            e.value.clone()
        })
    }

    /// 设置敏感值
    pub fn set(
        &self,
        key: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<(), VaultError> {
        // 验证key格式
        Self::validate_key(key)?;

        let mut entries = self.entries.write().unwrap();

        let entry = VaultEntry {
            key: key.to_string(),
            value: value.to_string(),
            description: description.map(|s| s.to_string()),
            created_at: entries.get(key)
                .map(|e| e.created_at)
                .unwrap_or_else(Utc::now),
            last_used: None,
        };

        entries.insert(key.to_string(), entry);
        self.persist(&entries)?;

        debug!("[Vault] Set entry: {}", key);
        Ok(())
    }

    /// 列出所有key的元数据
    pub fn list_keys(&self) -> Vec<VaultMetadata> {
        let entries = self.entries.read().unwrap();
        entries.values().map(|e| VaultMetadata::from(e)).collect()
    }

    /// 删除条目
    pub fn delete(&self, key: &str) -> Result<bool, VaultError> {
        let mut entries = self.entries.write().unwrap();

        if entries.remove(key).is_some() {
            self.persist(&entries)?;
            debug!("[Vault] Deleted entry: {}", key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 检查key是否存在
    pub fn exists(&self, key: &str) -> bool {
        let entries = self.entries.read().unwrap();
        entries.contains_key(key)
    }

    /// 持久化到磁盘
    fn persist(&self, entries: &HashMap<String, VaultEntry>) -> Result<(), VaultError> {
        if self.path.to_str() == Some(":memory:") {
            return Ok(());
        }

        let content = serde_json::to_string_pretty(entries)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// 验证key格式
    fn validate_key(key: &str) -> Result<(), VaultError> {
        if key.is_empty() {
            return Err(VaultError::InvalidKey("Key cannot be empty".to_string()));
        }
        if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(VaultError::InvalidKey(
                "Key must contain only alphanumeric characters and underscores".to_string()
            ));
        }
        Ok(())
    }
}
```

---

## 3. Placeholder Scanner

```rust
// src/vault/scanner.rs

use regex::Regex;
use std::collections::HashSet;

/// Placeholder模式: {{vault:key_name}}
pub const PLACEHOLDER_PATTERN: &str = r"\{\{vault:([a-zA-Z0-9_]+)\}\}";

/// 扫描结果
#[derive(Debug, Clone)]
pub struct Placeholder {
    /// 提取的key
    pub key: String,
    /// 完整匹配 ({{vault:key}})
    pub full_match: String,
    /// 起始位置
    pub start: usize,
    /// 结束位置
    pub end: usize,
}

/// 扫描文本中的placeholder
pub fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    lazy_static! {
        static ref RE: Regex = Regex::new(PLACEHOLDER_PATTERN).unwrap();
    }

    RE.captures_iter(text)
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

/// 提取所有唯一的key
pub fn extract_keys(text: &str) -> HashSet<String> {
    scan_placeholders(text)
        .into_iter()
        .map(|p| p.key)
        .collect()
}

/// 替换文本中的placeholder
pub fn replace_placeholders(text: &str, replacements: &std::collections::HashMap<String, String>) -> String {
    lazy_static! {
        static ref RE: Regex = Regex::new(PLACEHOLDER_PATTERN).unwrap();
    }

    RE.replace_all(text, |cap: &regex::Captures| {
        let key = &cap[1];
        replacements.get(key)
            .map(|s| s.as_str())
            .unwrap_or_else(|| cap.get(0).unwrap().as_str())
    }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_single() {
        let text = "使用 {{vault:db_password}} 连接数据库";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].key, "db_password");
        assert_eq!(result[0].full_match, "{{vault:db_password}}");
    }

    #[test]
    fn test_scan_multiple() {
        let text = "用 {{vault:key1}} 和 {{vault:key2}}";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].key, "key1");
        assert_eq!(result[1].key, "key2");
    }

    #[test]
    fn test_scan_none() {
        let text = "没有placeholder";
        let result = scan_placeholders(text);
        assert!(result.is_empty());
    }

    #[test]
    fn test_replace() {
        let mut replacements = std::collections::HashMap::new();
        replacements.insert("key".to_string(), "value".to_string());

        let text = "使用 {{vault:key}} 测试";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, "使用 value 测试");
    }

    #[test]
    fn test_replace_missing_key() {
        let replacements = std::collections::HashMap::new();
        let text = "使用 {{vault:missing}} 测试";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, text); // 保持原样
    }
}
```

---

## 4. VaultInjector

```rust
// src/vault/injector.rs

use std::sync::Arc;
use std::collections::{HashMap, HashSet};

use tracing::{debug, warn};

use crate::providers::ChatMessage;
use super::{VaultStore, scan_placeholders, replace_placeholders};

/// 注入报告
#[derive(Debug, Default)]
pub struct InjectionReport {
    /// 替换的消息数量
    pub messages_modified: usize,
    /// 使用的key列表
    pub keys_used: Vec<String>,
    /// 缺失的key列表
    pub missing_keys: Vec<String>,
    /// 所有注入的值 (用于日志过滤)
    pub injected_values: Vec<String>,
}

/// Vault注入器
///
/// 在发送给LLM前的最后时刻注入敏感数据
pub struct VaultInjector {
    store: Arc<VaultStore>,
}

impl VaultInjector {
    pub fn new(store: Arc<VaultStore>) -> Self {
        Self { store }
    }

    /// 注入所有消息中的placeholder
    ///
    /// 返回注入报告，包含使用的key和注入的值
    pub fn inject(&self, messages: &mut [ChatMessage]) -> InjectionReport {
        let mut report = InjectionReport::default();
        let mut all_keys = HashSet::new();

        // 1. 收集所有需要的key
        for msg in messages.iter() {
            for placeholder in scan_placeholders(&msg.content) {
                all_keys.insert(placeholder.key);
            }
        }

        if all_keys.is_empty() {
            return report;
        }

        debug!("[VaultInjector] Found {} unique keys to inject", all_keys.len());

        // 2. 构建替换映射
        let mut replacements = HashMap::new();
        for key in &all_keys {
            if let Some(value) = self.store.get(key) {
                replacements.insert(key.clone(), value.clone());
                report.keys_used.push(key.clone());
                report.injected_values.push(value);
            } else {
                report.missing_keys.push(key.clone());
                warn!("[VaultInjector] Key not found in vault: {}", key);
            }
        }

        // 3. 执行替换
        for msg in messages.iter_mut() {
            let original = &msg.content;
            let replaced = replace_placeholders(original, &replacements);
            if replaced != *original {
                report.messages_modified += 1;
                msg.content = replaced;
            }
        }

        debug!(
            "[VaultInjector] Injected {} values into {} messages",
            report.keys_used.len(),
            report.messages_modified
        );

        report
    }

    /// 仅扫描消息中的placeholder，不注入
    pub fn scan(&self, messages: &[ChatMessage]) -> HashSet<String> {
        let mut keys = HashSet::new();
        for msg in messages {
            for placeholder in scan_placeholders(&msg.content) {
                keys.insert(placeholder.key);
            }
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> Arc<VaultStore> {
        let store = VaultStore::new_in_memory();
        store.set("api_key", "sk-12345", Some("Test API key")).unwrap();
        store.set("password", "secret123", None).unwrap();
        Arc::new(store)
    }

    #[test]
    fn test_inject_single() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![
            ChatMessage::user("使用 {{vault:api_key}} 调用API"),
        ];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 1);
        assert_eq!(report.keys_used, vec!["api_key"]);
        assert!(report.missing_keys.is_empty());
        assert_eq!(messages[0].content, "使用 sk-12345 调用API");
    }

    #[test]
    fn test_inject_multiple_keys() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![
            ChatMessage::user("用 {{vault:api_key}} 和 {{vault:password}}"),
        ];

        let report = injector.inject(&mut messages);

        assert_eq!(report.keys_used.len(), 2);
        assert!(messages[0].content.contains("sk-12345"));
        assert!(messages[0].content.contains("secret123"));
    }

    #[test]
    fn test_missing_key() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![
            ChatMessage::user("使用 {{vault:unknown_key}}"),
        ];

        let report = injector.inject(&mut messages);

        assert_eq!(report.missing_keys, vec!["unknown_key"]);
        // 未知key保持原样
        assert_eq!(messages[0].content, "使用 {{vault:unknown_key}}");
    }
}
```

---

## 5. Redaction

```rust
// src/vault/redaction.rs

/// 替换敏感值为 [REDACTED]
pub fn redact_secrets(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    for secret in secrets {
        if !secret.is_empty() && text.contains(secret) {
            result = result.replace(secret, "[REDACTED]");
        }
    }
    result
}

/// 检查文本是否包含任何敏感值
pub fn contains_secrets(text: &str, secrets: &[String]) -> bool {
    secrets.iter().any(|s| !s.is_empty() && text.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact() {
        let text = "密码是 secret123 请保密";
        let secrets = vec!["secret123".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "密码是 [REDACTED] 请保密");
    }

    #[test]
    fn test_redact_multiple() {
        let text = "key=sk-12345 pass=secret123";
        let secrets = vec!["sk-12345".to_string(), "secret123".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "key=[REDACTED] pass=[REDACTED]");
    }

    #[test]
    fn test_no_secrets() {
        let text = "没有敏感数据";
        let secrets: Vec<String> = vec![];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, text);
    }
}
```

---

## 6. Module Exports

```rust
// src/vault/mod.rs

//! Vault: 敏感数据隔离模块
//!
//! 提供敏感数据的安全存储和运行时注入功能。
//!
//! # 设计原则
//!
//! 1. **数据结构隔离**: VaultStore与memory/history完全分离
//! 2. **运行时注入**: 敏感数据只在发送给LLM前一刻注入
//! 3. **零信任**: 敏感数据永不落盘到LLM可访问的存储
//!
//! # 使用方式
//!
//! ```ignore
//! use nanobot_core::vault::{VaultStore, VaultInjector};
//!
//! // 创建存储
//! let store = Arc::new(VaultStore::new()?);
//! store.set("api_key", "sk-12345", Some("OpenAI API key"))?;
//!
//! // 创建注入器
//! let injector = VaultInjector::new(store.clone());
//!
//! // 注入消息
//! let mut messages = vec![ChatMessage::user("使用 {{vault:api_key}}")];
//! let report = injector.inject(&mut messages);
//! // messages[0].content == "使用 sk-12345"
//! ```

mod error;
mod store;
mod scanner;
mod injector;
mod redaction;

pub use error::VaultError;
pub use store::{VaultStore, VaultEntry, VaultMetadata};
pub use scanner::{Placeholder, scan_placeholders, extract_keys, replace_placeholders, PLACEHOLDER_PATTERN};
pub use injector::{VaultInjector, InjectionReport};
pub use redaction::{redact_secrets, contains_secrets};
```

---

## 7. AgentLoop 集成

```rust
// 在 src/agent/loop_.rs 中添加

use crate::vault::{VaultStore, VaultInjector, InjectionReport, redact_secrets};

pub struct AgentLoop {
    // ... 现有字段 ...

    /// Vault注入器 (可选)
    vault_injector: Option<VaultInjector>,

    /// 存储注入的值 (用于日志过滤)
    vault_values: Arc<RwLock<Vec<String>>>,
}

impl AgentLoop {
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
    ) -> Result<Self, AgentError> {
        // ... 现有初始化代码 ...

        // 初始化Vault (如果存在)
        let vault_store = match VaultStore::new() {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                warn!("Failed to initialize vault: {}", e);
                None
            }
        };

        let vault_injector = vault_store.as_ref()
            .map(|s| VaultInjector::new(s.clone()));

        Ok(Self {
            // ... 现有字段 ...
            vault_injector,
            vault_values: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub async fn process_direct_with_callback(
        &self,
        content: &str,
        session_key: &SessionKey,
        callback: Option<&StreamCallback>,
    ) -> Result<AgentResponse, AgentError> {
        // ... 步骤1-6保持不变 ...

        // ── 7. Assemble prompt ─────────────────────────────────────
        let mut messages = Self::assemble_prompt(
            processed.messages,
            content,  // 原始输入，包含placeholder
            &system_prompts,
            summary.as_deref(),
        );

        // ── 7.5. Inject vault secrets ──────────────────────────────
        let injection_report = if let Some(ref injector) = self.vault_injector {
            let report = injector.inject(&mut messages);

            // 存储注入的值用于日志过滤
            *self.vault_values.write().unwrap() = report.injected_values.clone();

            if !report.missing_keys.is_empty() {
                warn!("[Vault] Missing keys: {:?}", report.missing_keys);
            }
            if !report.keys_used.is_empty() {
                debug!("[Vault] Injected {} keys: {:?}",
                       report.keys_used.len(), report.keys_used);
            }

            Some(report)
        } else {
            None
        };

        // ── 8. Run agent loop ─────────────────────────────────────
        let result = self.run_agent_loop(messages, effective_cb).await?;

        // ... 步骤9保持不变 ...

        // ── 10. Save assistant message (清理敏感数据) ───────────────
        if let Some(ref sm) = self.session_manager {
            let safe_content = if let Some(ref report) = injection_report {
                redact_secrets(&result.content, &report.injected_values)
            } else {
                result.content.clone()
            };

            sm.append_by_key(
                session_key,
                "assistant",
                &safe_content,
                Some(result.tools_used.clone()),
            ).await?;
        }

        Ok(AgentResponse {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
        })
    }
}

// 修改日志函数
fn log_llm_response(
    response: &ChatResponse,
    iteration: u32,
    vault_values: &[String],
) {
    let content = response.content.as_ref().map(|c| {
        redact_secrets(c, vault_values)
    });

    if let Some(ref reasoning) = response.reasoning_content {
        if !reasoning.is_empty() {
            let safe_reasoning = redact_secrets(reasoning, vault_values);
            debug!("[Agent] Reasoning (iter {}): {}", iteration, safe_reasoning);
        }
    }

    if let Some(ref c) = content {
        if !c.is_empty() {
            info!("[Agent] Response (iter {}): {}", iteration, c);
        }
    }
}
```

---

## 8. CLI 集成

```rust
// src/commands/vault.rs

use clap::Subcommand;
use nanobot_core::vault::{VaultStore, VaultMetadata};

#[derive(Debug, Subcommand)]
pub enum VaultCommand {
    /// 设置敏感数据
    Set {
        /// Key名称
        key: String,
        /// 描述
        #[arg(short, long)]
        description: Option<String>,
        /// 值 (如果不提供，会交互式输入)
        #[arg(short, long)]
        value: Option<String>,
    },

    /// 获取敏感数据元信息
    Get {
        /// Key名称
        key: String,
        /// 显示明文值 (危险)
        #[arg(long)]
        show: bool,
    },

    /// 列出所有key
    List,

    /// 删除敏感数据
    Delete {
        /// Key名称
        key: String,
    },
}

pub fn execute(cmd: VaultCommand) -> Result<(), Box<dyn std::error::Error>> {
    let store = VaultStore::new()?;

    match cmd {
        VaultCommand::Set { key, description, value } => {
            let value = match value {
                Some(v) => v,
                None => {
                    // 交互式输入
                    rpassword::prompt_password("Enter value: ")?
                }
            };
            store.set(&key, &value, description.as_deref())?;
            println!("✓ Set: {}", key);
        }

        VaultCommand::Get { key, show } => {
            if !store.exists(&key) {
                println!("✗ Key not found: {}", key);
                return Ok(());
            }

            if show {
                if let Some(value) = store.get(&key) {
                    println!("Key: {}", key);
                    println!("Value: {}", value);
                }
            } else {
                // 只显示元数据
                let keys = store.list_keys();
                if let Some(meta) = keys.iter().find(|m| m.key == key) {
                    println!("Key: {}", meta.key);
                    if let Some(ref desc) = meta.description {
                        println!("Description: {}", desc);
                    }
                    println!("Created: {}", meta.created_at);
                    if let Some(ref last) = meta.last_used {
                        println!("Last used: {}", last);
                    }
                    println!("Value: ******** (hidden, use --show to reveal)");
                }
            }
        }

        VaultCommand::List => {
            let keys = store.list_keys();
            if keys.is_empty() {
                println!("No vault entries found.");
                return Ok(());
            }

            println!("Vault entries:");
            for meta in keys {
                let desc = meta.description
                    .as_ref()
                    .map(|s| format!(" - {}", s))
                    .unwrap_or_default();
                println!("  {}{}", meta.key, desc);
            }
        }

        VaultCommand::Delete { key } => {
            if store.delete(&key)? {
                println!("✓ Deleted: {}", key);
            } else {
                println!("✗ Key not found: {}", key);
            }
        }
    }

    Ok(())
}
```

---

## 数据流验证

```
用户输入: "用 {{vault:db_pass}} 连接数据库"
     │
     ▼
┌────────────────────────────────────┐
│ Step 3: 保存用户消息                │
│ content = "用 {{vault:db_pass}}..." │  ← 保存placeholder
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐
│ Step 7: assemble_prompt            │
│ messages[0].content = "{{vault:..}}"│
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐
│ Step 7.5: vault_injector.inject()  │
│ messages[0].content = "real_pass"  │  ← 注入明文
│ report.injected_values = ["real"]  │
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐
│ Step 8: run_agent_loop             │
│ LLM收到: "用 real_pass 连接数据库"  │  ← LLM看到明文
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐
│ Step 10: 保存assistant消息          │
│ safe_content = redact_secrets(...) │  ← 清理敏感数据
│ 保存: "...[REDACTED]..."           │
└────────────────────────────────────┘

验证点:
1. SQLite session_messages: 只有placeholder和[REDACTED]
2. Markdown memory files: 只有placeholder和[REDACTED]
3. 日志: 只有placeholder和[REDACTED]
4. LLM上下文: 短暂存在明文，用完即弃
```
