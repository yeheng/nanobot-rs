# Vault: 敏感数据隔离方案

## 状态
- **提议日期**: 2026-03-06
- **状态**: Draft
- **作者**: Linus-style Design Review

---

## Linus式问题分析

### 三个核心问题

```
1. "这是个真问题还是臆想出来的？"
   → 真问题。LLM可能泄露敏感数据到日志、输出或其他地方。
   → 现有架构没有隔离机制，敏感数据散落在memory/history中。

2. "有更简单的方法吗？"
   → 最简方案：敏感数据永不在存储中出现，只在运行时注入。
   → 不需要复杂的加密管道，只需要存储隔离+占位符替换。

3. "会破坏什么吗？"
   → 新组件，零破坏性。
   → 现有memory/history不受影响。
```

### 数据结构分析

```
"Bad programmers worry about the code. Good programmers worry about data structures."

当前问题：
- 敏感数据和非敏感数据混在一起
- 没有所有权概念
- LLM上下文直接包含所有数据

解决方案：
- VaultStore: 完全独立的存储
- Placeholder: 上下文中只存在占位符
- Runtime Injector: 最终时刻才注入明文
```

---

## 核心设计

### 一句话本质

**敏感数据通过vault按需注入，永不在存储中明文出现。**

### 三个核心概念

1. **VaultStore** - 敏感数据的安全存储（独立于memory/history）
2. **Placeholder** - 上下文中的占位符 `{{vault:key_name}}`
3. **VaultInjector** - 运行时注入器（在发送给LLM前一刻注入）

### 数据流

```
用户输入: "用 {{vault:db_password}} 连接数据库"
           │
           ▼
    ┌──────────────────┐
    │ assemble_prompt  │  ← 检测占位符
    └────────┬─────────┘
             │
             ▼
    ┌──────────────────┐
    │  VaultInjector   │  ← 交互式获取敏感数据
    └────────┬─────────┘
             │
             ▼
    发送给LLM: "用 real_password_here 连接数据库"
             │
             ▼
    ┌──────────────────┐
    │  LLM Response    │  ← LLM处理
    └────────┬─────────┘
             │
             ▼
    保存历史: "用 {{vault:db_password}} 连接数据库"  ← 保存占位符，不保存明文
```

---

## 组件设计

### 1. VaultStore

**位置**: `~/.nanobot/vault/`

```
vault/
├── secrets.json      # 加密的敏感数据 (可选加密)
├── metadata.json     # 元数据 (key名、描述)
└── .gitignore        # 确保不被git追踪
```

**数据结构**:

```rust
/// 敏感条目
pub struct VaultEntry {
    /// 唯一标识符 (用于placeholder: {{vault:key}})
    pub key: String,
    /// 敏感值 (内存中明文，存储时加密)
    pub value: String,
    /// 描述 (用于UI提示)
    pub description: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后使用时间
    pub last_used: Option<DateTime<Utc>>,
}

/// Vault存储
pub struct VaultStore {
    /// 存储路径
    path: PathBuf,
    /// 内存缓存 (可选，用于性能)
    cache: HashMap<String, VaultEntry>,
    /// 是否启用加密
    encrypted: bool,
}
```

**核心方法**:

```rust
impl VaultStore {
    /// 获取敏感值 (运行时调用)
    pub fn get(&self, key: &str) -> Result<Option<String>, VaultError>;

    /// 设置敏感值 (用户配置)
    pub fn set(&mut self, key: &str, value: &str, description: Option<&str>) -> Result<(), VaultError>;

    /// 列出所有key (用于UI)
    pub fn list_keys(&self) -> Result<Vec<VaultMetadata>, VaultError>;

    /// 删除条目
    pub fn delete(&mut self, key: &str) -> Result<(), VaultError>;
}
```

### 2. Placeholder格式

**语法**: `{{vault:key_name}}`

**示例**:
```
用户: "请用 {{vault:aws_access_key}} 和 {{vault:aws_secret_key}} 配置AWS CLI"
用户: "数据库连接字符串是 {{vault:db_connection_string}}"
用户: "API密钥 {{vault:openai_api_key}} 已过期，请更新"
```

**检测规则**:
- 正则: `\{\{vault:([a-zA-Z0-9_]+)\}\}`
- 大小写敏感
- 不支持嵌套

### 3. VaultInjector

**位置**: `assemble_prompt()` 之后，`run_agent_loop()` 之前

```rust
pub struct VaultInjector {
    store: Arc<VaultStore>,
    /// 是否需要交互式确认
    interactive: bool,
}

impl VaultInjector {
    /// 扫描并替换所有placeholder
    ///
    /// 如果是首次使用某个key，会触发交互式输入
    pub async fn inject(&self, messages: &mut [ChatMessage]) -> Result<InjectionReport, VaultError>;
}

pub struct InjectionReport {
    /// 替换的placeholder数量
    pub replaced: usize,
    /// 涉及的key列表
    pub keys_used: Vec<String>,
    /// 首次使用的key (需要用户输入)
    pub new_keys: Vec<String>,
}
```

### 4. VaultTool (可选)

让Agent能够请求敏感数据（需要用户确认）:

```rust
pub struct VaultGetTool {
    store: Arc<VaultStore>,
}

impl Tool for VaultGetTool {
    fn name(&self) -> &str { "vault_get" }

    fn description(&self) -> &str {
        "获取存储在vault中的敏感数据。需要用户确认。"
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let key = args["key"].as_str().ok_or(...)?;

        // 检查权限 (需要用户确认)
        // 返回值 (不记录到日志)
    }
}
```

---

## 集成点

### AgentLoop 修改

```rust
// 在 assemble_prompt() 之后添加注入步骤

// ── 7. Assemble prompt (pure, synchronous) ─────────────────
let mut messages = Self::assemble_prompt(
    processed.messages,
    content,
    &system_prompts,
    summary.as_deref(),
);

// ── 7.5. Inject vault secrets (NEW) ────────────────────────
if let Some(ref injector) = self.vault_injector {
    let report = injector.inject(&mut messages).await?;
    if !report.keys_used.is_empty() {
        debug!("[Vault] Injected {} secrets: {:?}",
               report.replaced, report.keys_used);
    }
}

// ── 8. Run agent loop ─────────────────────────────────────
let result = self.run_agent_loop(messages, effective_cb).await?;
```

### 历史保存修改

**关键**: 保存历史时使用原始消息（带placeholder），而不是注入后的消息。

```rust
// ── 3. Save user message ───────────────────────────────────
// 保存原始消息 (带 {{vault:*}} placeholder)
if let Some(ref sm) = self.session_manager {
    sm.append_by_key(session_key, "user", content, None).await?;
    // content 是原始输入，包含 {{vault:*}} 而非明文
}
```

---

## 安全考虑

### 1. 敏感数据永不落盘到LLM可访问的位置

```
✗ SQLite session_messages - 只有placeholder
✗ Markdown memory files - 只有placeholder
✗ 日志/trace - 只有placeholder
✗ Tantivy索引 - 只有placeholder

✓ VaultStore - 独立加密存储
✓ 运行时内存 - 临时存在，用完即弃
```

### 2. 日志安全

```rust
// 在 log_llm_response 中过滤敏感数据
fn log_llm_response(response: &ChatResponse, iteration: u32, vault_keys: &[String]) {
    let content = response.content.as_ref().map(|c| {
        // 替换敏感数据为 [REDACTED]
        redact_secrets(c, vault_keys)
    });
    // ...
}
```

### 3. 工具返回值处理

```rust
// 工具返回值可能包含敏感数据
// 在保存到历史前进行清理
fn sanitize_tool_output(output: &str, vault_keys: &[String]) -> String {
    redact_secrets(output, vault_keys)
}
```

---

## 配置

### ~/.nanobot/vault.toml

```toml
[vault]
# 是否启用加密存储
encrypted = true

# 加密方式: "password" | "keychain" | "none"
encryption_method = "keychain"

# 是否在注入时显示确认提示
confirm_on_inject = true

# 会话结束后是否清除内存缓存
clear_cache_on_session_end = true
```

---

## 实现步骤

### Phase 1: 核心功能 (MVP)

1. **VaultStore** - 基础存储 (无加密)
2. **Placeholder扫描** - 正则检测
3. **VaultInjector** - 运行时注入
4. **集成到AgentLoop** - 在assemble_prompt后注入

### Phase 2: 安全增强

1. **加密存储** - 使用系统keychain或密码加密
2. **日志过滤** - 自动替换敏感数据
3. **交互式配置** - 首次使用时提示输入

### Phase 3: 用户体验

1. **VaultTool** - 让Agent能请求敏感数据
2. **CLI命令** - `nanobot vault set/get/list`
3. **UI集成** - 配置界面

---

## 测试用例

### 1. 基本功能

```rust
#[test]
fn test_placeholder_detection() {
    let text = "使用 {{vault:db_password}} 连接数据库";
    let placeholders = scan_placeholders(text);
    assert_eq!(placeholders, vec!["db_password"]);
}

#[test]
fn test_injection() {
    let mut store = VaultStore::new_in_memory();
    store.set("db_password", "secret123", None).unwrap();

    let injector = VaultInjector::new(Arc::new(store));
    let mut messages = vec![ChatMessage::user("使用 {{vault:db_password}}")];

    injector.inject(&mut messages).await.unwrap();

    assert_eq!(messages[0].content, "使用 secret123");
}
```

### 2. 历史保存

```rust
#[test]
fn test_history_preserves_placeholders() {
    // 用户输入包含placeholder
    let user_input = "用 {{vault:api_key}} 调用API";

    // 注入后发送给LLM
    let injected = inject_secrets(user_input); // "用 real_key 调用API"

    // 保存到历史的是原始输入
    let saved = get_saved_history();
    assert!(saved.contains("{{vault:api_key}}"));
    assert!(!saved.contains("real_key"));
}
```

### 3. 安全性

```rust
#[test]
fn test_secrets_not_in_memory_search() {
    // 设置敏感数据
    vault.set("api_key", "sk-12345", None);

    // 使用敏感数据
    process("使用 {{vault:api_key}}").await;

    // 搜索memory不应找到明文
    let results = memory_search("sk-12345").await;
    assert!(results.is_empty());
}
```

---

## 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 用户误将明文写入memory | 敏感数据泄露 | 提供扫描工具，检测memory中的敏感模式 |
| LLM输出包含敏感数据 | 间接泄露 | 后处理过滤，替换已知敏感值 |
| 工具返回敏感数据 | 落入历史 | 在保存前清理工具输出 |
| 加密密钥丢失 | 数据不可恢复 | 支持导出/导入，警告用户备份 |

---

## 拒绝的设计

以下设计方案被明确拒绝：

### ❌ 方案A: 自动检测敏感数据

```
"这是在解决不存在的问题。"

问题：什么是"敏感数据"？没有明确边界。
结果：误报、漏报、复杂的规则引擎。
正确做法：用户显式标记 {{vault:*}}
```

### ❌ 方案B: 全局加密所有存储

```
"复杂性是万恶之源。"

问题：加密所有memory/history增加复杂度。
结果：性能下降、调试困难、密钥管理问题。
正确做法：只加密真正敏感的数据，隔离存储。
```

### ❌ 方案C: 运行时脱敏

```
"事后补丁是糟糕设计的标志。"

问题：敏感数据已经进入系统，脱敏是补丁。
结果：遗漏点、性能开销、复杂的数据流。
正确做法：敏感数据从一开始就不在那些地方。
```

---

## 总结

这是一个**实用主义**的解决方案：

1. **简单** - 占位符 + 运行时注入，没有复杂的加密管道
2. **安全** - 敏感数据永不在LLM可访问的存储中出现
3. **无破坏** - 现有系统不受影响，新功能增量添加
4. **可扩展** - 后续可添加加密、UI、工具等

```
"Talk is cheap. Show me the code."
```

下一步：实现 Phase 1 MVP。
