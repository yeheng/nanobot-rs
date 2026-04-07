# Cron 文件驱动架构重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Cron 系统从 SQLite 持久化重构为文件驱动架构，以 Markdown 作为 SSOT，支持热重载

**Architecture:** 
- 移除 `storage/src/cron.rs` 中的 SQLite cron 代码
- 重构 `engine/src/cron/service.rs` 为纯内存存储，使用 `notify` crate 监听文件变化
- Markdown + Frontmatter 格式作为 job 定义文件格式
- CLI 工具 `CronTool` 改为直接操作 `.md` 文件

**Tech Stack:** Rust, notify 7.x, serde_yaml (frontmatter 解析), cron crate

---

## File Structure

**修改的文件:**
- `storage/src/cron.rs` - 移除所有 cron 相关代码 (整个文件可删除)
- `storage/src/lib.rs` - 移除 cron 模块导出
- `engine/src/cron/service.rs` - 重构为内存存储 + 文件监听
- `engine/src/tools/cron.rs` - 修改为文件操作
- `engine/Cargo.toml` - 添加 notify 依赖

**新增的文件:**
- `~/.gasket/cron/*.md` - Markdown job 定义文件 (用户编辑)

**删除的文件:**
- `storage/src/cron.rs` - SQLite cron 持久化代码

---

## Task 1: 添加 notify 依赖到 engine crate

**Files:**
- Modify: `engine/Cargo.toml`

- [ ] **Step 1: 读取 engine/Cargo.toml 当前依赖**

```bash
cat engine/Cargo.toml
```

- [ ] **Step 2: 添加 notify 依赖**

在 `[dependencies]` 部分添加：

```toml
notify = "7"
```

- [ ] **Step 3: 验证依赖添加成功**

```bash
cargo check -p gasket-engine
```
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add engine/Cargo.toml
git commit -m "chore(engine): add notify dependency for cron file watching"
```

---

## Task 2: 移除 storage/src/cron.rs 中的 SQLite cron 代码

**Files:**
- Delete: `storage/src/cron.rs`
- Modify: `storage/src/lib.rs`

- [ ] **Step 1: 读取 storage/src/lib.rs 检查 cron 模块导出**

```bash
grep -n "cron" storage/src/lib.rs
```

- [ ] **Step 2: 从 storage/src/lib.rs 移除 cron 模块导出**

删除类似以下的行：
```rust
mod cron;
pub use cron::CronJobRow;
```

- [ ] **Step 3: 删除 storage/src/cron.rs 文件**

```bash
rm storage/src/cron.rs
```

- [ ] **Step 4: 验证 storage crate 编译**

```bash
cargo check -p gasket-storage
```
Expected: 编译通过（如有其他模块引用 cron 代码会报错，需要记录）

- [ ] **Step 5: Commit**

```bash
git add storage/src/lib.rs
git rm storage/src/cron.rs
git commit -m "refactor(storage): remove sqlite cron persistence module"
```

---

## Task 3: 重构 CronService 为内存存储 + 文件解析

**Files:**
- Modify: `engine/src/cron/service.rs`

- [ ] **Step 1: 读取当前 service.rs 完整内容**

```bash
cat engine/src/cron/service.rs
```

- [ ] **Step 2: 定义 CronJob 结构（保留，移除 From<CronJobRow>）**

```rust
/// A scheduled job
#[derive(Debug, Clone)]
pub struct CronJob {
    /// Unique job ID (filename without .md)
    pub id: String,
    /// Job name
    pub name: String,
    /// Cron expression
    pub cron: String,
    /// Message to send
    pub message: String,
    /// Target channel
    pub channel: Option<String>,
    /// Target chat ID
    pub chat_id: Option<String>,
    /// Next run time (in-memory only)
    pub next_run: Option<DateTime<Utc>>,
    /// Enabled
    pub enabled: bool,
    /// File path for hot reload
    pub file_path: PathBuf,
}
```

- [ ] **Step 3: 定义 CronService 结构（纯内存）**

```rust
use notify::{RecommendedWatcher, Watcher, RecursiveMode};
use std::sync::mpsc::{channel, Receiver};
use parking_lot::RwLock;

pub struct CronService {
    /// In-memory job storage
    jobs: RwLock<HashMap<String, CronJob>>,
    /// Workspace path
    workspace: PathBuf,
    /// File watcher
    watcher: RwLock<Option<RecommendedWatcher>>,
    /// Watcher event receiver
    rx: Receiver<notify::Result<notify::Event>>,
}
```

- [ ] **Step 4: 实现 parse_markdown 函数**

```rust
/// Parse markdown file with frontmatter
fn parse_markdown(content: &str, file_path: &Path) -> anyhow::Result<CronJob> {
    // Split frontmatter and body
    let parts: Vec<&str> = content.splitn(2, "---\n").collect();
    if parts.len() < 3 {
        anyhow::bail!("Invalid markdown format: missing frontmatter delimiters");
    }
    
    // Parse frontmatter (parts[1])
    let fm: CronJobFrontmatter = serde_yaml::from_str(parts[1])?;
    
    // Body is parts[2]
    let message = parts.get(2).unwrap_or(&"").trim().to_string();
    
    // Calculate next_run
    let next_run = calculate_next_run(&fm.cron);
    
    // Use filename as ID
    let id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    
    Ok(CronJob {
        id: id.clone(),
        name: fm.name.unwrap_or(id),
        cron: fm.cron,
        message,
        channel: fm.channel,
        chat_id: fm.to,
        next_run,
        enabled: fm.enabled,
        file_path: file_path.to_path_buf(),
    })
}

#[derive(Debug, Deserialize)]
struct CronJobFrontmatter {
    name: Option<String>,
    cron: String,
    channel: Option<String>,
    to: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}
```

- [ ] **Step 5: 实现 CronService::new()**

```rust
impl CronService {
    pub async fn new(workspace: PathBuf) -> Self {
        let (tx, rx) = channel();
        
        let service = Self {
            jobs: RwLock::new(HashMap::new()),
            workspace: workspace.clone(),
            watcher: RwLock::new(None),
            rx,
        };
        
        // Load existing jobs
        service.load_all_jobs(&workspace);
        
        // Start file watcher
        service.start_watcher(tx);
        
        service
    }
    
    fn load_all_jobs(&self, workspace: &Path) {
        let cron_dir = workspace.join("cron");
        if !cron_dir.exists() {
            let _ = std::fs::create_dir_all(&cron_dir);
            return;
        }
        
        for entry in std::fs::read_dir(&cron_dir).ok().into_iter().flatten() {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "md") {
                    if let Ok(job) = Self::parse_markdown_file(&path) {
                        self.jobs.write().insert(job.id.clone(), job);
                    }
                }
            }
        }
    }
    
    fn parse_markdown_file(path: &Path) -> anyhow::Result<CronJob> {
        let content = std::fs::read_to_string(path)?;
        parse_markdown(&content, path)
    }
    
    fn start_watcher(&self, tx: std::sync::mpsc::Sender<notify::Result<notify::Event>>) {
        let cron_dir = self.workspace.join("cron");
        
        let mut watcher = RecommendedWatcher::new(tx, notify::Config::default()).ok();
        
        if let Some(ref mut w) = watcher {
            let _ = w.watch(&cron_dir, RecursiveMode::NonRecursive);
        }
        
        *self.watcher.write() = watcher;
    }
}
```

- [ ] **Step 6: 实现文件事件处理**

```rust
impl CronService {
    /// Poll watcher and update jobs
    pub fn poll_watcher(&self) {
        while let Ok(event_result) = self.rx.try_recv() {
            if let Ok(event) = event_result {
                for path in &event.paths {
                    if path.extension().is_some_and(|ext| ext == "md") {
                        match event.kind {
                            notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                                if let Ok(job) = Self::parse_markdown_file(path) {
                                    self.jobs.write().insert(job.id.clone(), job);
                                }
                            }
                            notify::EventKind::Remove(_) => {
                                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                                    self.jobs.write().remove(id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 7: 实现 list_jobs 和 get_due_jobs**

```rust
impl CronService {
    pub async fn list_jobs(&self) -> Vec<CronJob> {
        self.jobs.read().values().cloned().collect()
    }
    
    pub async fn get_due_jobs(&self) -> Vec<CronJob> {
        let now = Utc::now();
        self.poll_watcher(); // Update from file changes
        
        self.jobs.read()
            .values()
            .filter(|job| {
                job.enabled && job.next_run.is_some_and(|nr| nr <= now)
            })
            .cloned()
            .collect()
    }
    
    /// Check if any job should execute immediately on startup
    pub fn should_execute_on_startup(&self, job: &CronJob) -> bool {
        job.next_run.is_some_and(|nr| nr <= Utc::now())
    }
}
```

- [ ] **Step 8: 移除 mark_job_run 方法（不再需要）**

删除 `mark_job_run()` 函数

- [ ] **Step 9: 验证 engine crate 编译**

```bash
cargo check -p gasket-engine
```

- [ ] **Step 10: Commit**

```bash
git add engine/src/cron/service.rs
git commit -m "refactor(engine): migrate CronService to in-memory file-driven architecture"
```

---

## Task 4: 更新 CronTool 支持文件操作

**Files:**
- Modify: `engine/src/tools/cron.rs`

- [ ] **Step 1: 修改 add 动作 - 创建 .md 文件**

```rust
"add" => {
    let name = args.name.ok_or_else(|| {
        ToolError::InvalidArguments("name is required for add".to_string())
    })?;
    let cron = args.cron.ok_or_else(|| {
        ToolError::InvalidArguments("cron is required for add".to_string())
    })?;
    let message = args.message.ok_or_else(|| {
        ToolError::InvalidArguments("message is required for add".to_string())
    })?;

    // Validate cron expression
    let _: cron::Schedule = cron.parse().map_err(|e| {
        ToolError::InvalidArguments(format!("Invalid cron expression: {}", e))
    })?;

    // Create markdown file
    let workspace = /* get workspace path */;
    let cron_dir = workspace.join("cron");
    let file_path = cron_dir.join(format!("{}.md", name));
    
    let content = format!(
        "---
name: {}
cron: "{}"
channel: {}
to: {}
enabled: true
---

{}",
        name,
        cron,
        channel.unwrap_or_default(),
        chat_id.unwrap_or_default(),
        message
    );
    
    std::fs::write(&file_path, content).map_err(|e| {
        ToolError::ExecutionError(format!("Failed to create cron file: {}", e))
    })?;

    Ok(format!("Scheduled job '{}' with ID: {}", name, name))
}
```

- [ ] **Step 2: 修改 remove 动作 - 删除 .md 文件**

```rust
"remove" => {
    let job_id = args.job_id.ok_or_else(|| {
        ToolError::InvalidArguments("job_id is required for remove".to_string())
    })?;

    let workspace = /* get workspace path */;
    let file_path = workspace.join("cron").join(format!("{}.md", job_id));
    
    if !file_path.exists() {
        return Ok(format!("Job not found: {}", job_id));
    }
    
    std::fs::remove_file(&file_path).map_err(|e| {
        ToolError::ExecutionError(format!("Failed to remove cron file: {}", e))
    })?;

    Ok(format!("Removed job: {}", job_id))
}
```

- [ ] **Step 3: list 动作保持不变（从内存读取）**

- [ ] **Step 4: 验证编译**

```bash
cargo check -p gasket-engine
```

- [ ] **Step 5: Commit**

```bash
git add engine/src/tools/cron.rs
git commit -m "refactor(engine): update CronTool to operate on markdown files"
```

---

## Task 5: 更新 gateway.rs 中的 cron 启动逻辑

**Files:**
- Modify: `cli/src/commands/gateway.rs`

- [ ] **Step 1: 移除 SqliteStore 依赖**

原代码：
```rust
let memory_store = Arc::new(MemoryStore::new().await);
let sqlite_store = memory_store.sqlite_store().clone();
let cron_service = Arc::new(CronService::with_store(sqlite_store.clone(), workspace.clone()).await);
```

修改为：
```rust
let cron_service = Arc::new(CronService::new(workspace.clone()).await);
```

- [ ] **Step 2: 移除 sqlite_store 传递给 tool_registry**

原代码：
```rust
sqlite_store: Some(sqlite_store),
```

修改为：
```rust
sqlite_store: None,
```

- [ ] **Step 3: 验证编译**

```bash
cargo check -p gasket-cli
```

- [ ] **Step 4: Commit**

```bash
git add cli/src/commands/gateway.rs
git commit -m "refactor(cli): update gateway to use file-driven CronService"
```

---

## Task 6: 迁移现有 YAML 文件到 Markdown 格式

**Files:**
- Create: `scripts/migrate_cron_yaml.sh`

- [ ] **Step 1: 创建迁移脚本**

```bash
#!/bin/bash
# Migrate cron YAML files to markdown format

CRON_DIR="${HOME}/.gasket/cron"

if [ ! -d "$CRON_DIR" ]; then
    echo "Cron directory does not exist: $CRON_DIR"
    exit 0
fi

for yaml_file in "$CRON_DIR"/*.yaml; do
    if [ -f "$yaml_file" ]; then
        name=$(basename "$yaml_file" .yaml)
        md_file="$CRON_DIR/${name}.md"
        
        echo "Migrating $yaml_file -> $md_file"
        
        cat > "$md_file" << EOF
---
$(cat "$yaml_file")
---

EOF
        
        # Optionally remove old yaml file
        # rm "$yaml_file"
    fi
done

echo "Migration complete!"
```

- [ ] **Step 2: 执行迁移脚本**

```bash
chmod +x scripts/migrate_cron_yaml.sh
./scripts/migrate_cron_yaml.sh
```

- [ ] **Step 3: 验证迁移结果**

```bash
cat ~/.gasket/cron/morning-weather.md
```

- [ ] **Step 4: Commit**

```bash
git add scripts/migrate_cron_yaml.sh
git commit -m "feat(scripts): add cron yaml to markdown migration script"
```

---

## Task 7: 集成测试与验证

**Files:**
- Test: Manual verification

- [ ] **Step 1: 构建全项目**

```bash
cargo build --workspace
```

- [ ] **Step 2: 启动 gateway 验证 cron 加载**

```bash
cargo run --release --package gasket-cli -- gateway
```

Expected output:
```
Loaded cron jobs from YAML: 1
```

- [ ] **Step 3: 验证 cron job 执行**

观察 log 输出，确认 cron job 被触发

- [ ] **Step 4: 测试 CLI 添加 job**

```bash
cargo run -- agent -m "/cron add name=test-job cron=*/1 * * * * message=test"
```

- [ ] **Step 5: 验证文件被创建**

```bash
cat ~/.gasket/cron/test-job.md
```

- [ ] **Step 6: 测试文件热重载**

```bash
# Edit the markdown file
echo "---
name: test-job
cron: \"*/1 * * * *\"
enabled: false
---

test message updated" > ~/.gasket/cron/test-job.md

# Wait for watcher to pick up changes (up to 60s)
# Verify job is disabled
```

- [ ] **Step 7: 测试 CLI 删除 job**

```bash
cargo run -- agent -m "/cron remove job_id=test-job"
```

- [ ] **Step 8: 验证文件被删除**

```bash
ls ~/.gasket/cron/test-job.md 2>&1
```
Expected: `No such file or directory`

---

## Task 8: 清理与文档

**Files:**
- Modify: `docs/architecture.md` (if exists)
- Create: `docs/cron-usage.md`

- [ ] **Step 1: 更新架构文档**

更新 `docs/architecture.md` 中关于 cron 系统的描述

- [ ] **Step 2: 创建 cron 使用文档**

```markdown
# Cron 使用指南

## 文件格式

Cron job 定义存储在 `~/.gasket/cron/*.md` 文件中，使用 Markdown + Frontmatter 格式。

### 示例

```markdown
---
name: morning-weather
cron: "*/10 * * * *"
channel: telegram
to: "8281248569"
enabled: true
---

请获取未来三天广州天气情况并发送给用户
```

### 字段说明

| 字段 | 必填 | 说明 |
|------|------|------|
| `name` | 否 | Job 名称（默认使用文件名） |
| `cron` | 是 | Cron 表达式 |
| `channel` | 否 | 目标渠道 |
| `to` | 否 | 目标用户 ID |
| `enabled` | 否 | 是否启用，默认 true |
| Body | 是 | 触发时发送的消息 |

## CLI 命令

### 添加 job

```
/cron add name=job-name cron="0 9 * * *" message="Hello"
```

### 列出 job

```
/cron list
```

### 删除 job

```
/cron remove job_id=job-name
```

## 热重载

修改 `.md` 文件后，CronService 会在下次轮询时（最多 60 秒）自动加载变更。
```

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: add cron usage guide and update architecture docs"
```

---

## 验收检查清单

- [ ] Gateway 启动后，`cron/*.md` 文件被正确加载
- [ ] 到期的 cron job 在启动时立即执行一次
- [ ] 修改 `enabled: false` 后 60s 内 job 停止执行
- [ ] CLI `cron add` 创建 `.md` 文件
- [ ] CLI `cron remove` 删除 `.md` 文件
- [ ] 所有 SQLite cron 相关代码已移除
- [ ] `cargo test --workspace` 通过
- [ ] `cargo clippy --workspace` 无警告

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-07-cron-file-driven-implementation.md`.

**Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
