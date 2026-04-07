# Cron 系统重构：文件驱动架构设计

**日期**: 2026-04-07  
**作者**: Gasket Team  
**状态**: Approved

---

## 1. 概述

### 1.1 背景

当前 Cron 系统使用 SQLite 作为 Single Source of Truth (SSOT)，存在以下问题：

1. **状态同步复杂性** - YAML 文件定义 job，启动时同步到 SQLite，运行时更新 `next_run`，存在双状态不一致风险
2. **next_run 为 NULL 的 bug** - 当前 `morning-weather` job 的 `next_run` 字段为 NULL，导致永远无法被查询到
3. **不必要的持久化** - cron 配置本质是静态定义，运行时状态（last_run, next_run）无需长期存储

### 1.2 目标

- 以 Markdown 文件作为 SSOT，移除 SQLite 依赖
- 简化架构，避免状态同步问题
- 支持文件热重载（无需重启 gateway）
- 保留 CLI 工具操作能力

### 1.3 非目标

- 不支持历史执行记录追踪（需要时可单独扩展）
- 不补偿错过的历史执行（重启后只执行当前到期的任务）

---

## 2. 架构设计

### 2.1 系统架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                    Cron 系统架构 (文件驱动)                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ~/.gasket/cron/                                                │
│  ├── morning-weather.md  ← SSOT (用户编辑)                      │
│  └── daily-report.md     ← SSOT (用户编辑)                      │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  CronService (内存状态)                                  │   │
│  │  ├─ watch_files() - notify 监视文件变化                  │   │
│  │  ├─ parse_markdown() - 解析 frontmatter + body          │   │
│  │  └─ jobs: HashMap<id, InMemoryJob> ← 运行时状态          │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  CronChecker (每 60s 轮询)                                  │   │
│  │  └─ for job in memory_jobs: if job.next_run <= now      │   │
│  │      ├─ publish_inbound(message)                         │   │
│  │      └─ job.update_next_run() ← 仅内存更新               │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 组件职责

| 组件 | 职责 | 依赖 |
|------|------|------|
| `CronService` | 加载/解析 Markdown 文件，维护内存 job 列表 | `notify`, `frontmatter` |
| `CronChecker` | 每 60s 轮询内存 jobs，触发到期任务 | `CronService` |
| `CronTool` | CLI 工具，支持添加/删除/列表操作 | `CronService` |
| `MarkdownParser` | 解析 frontmatter + body 格式 | `serde_yaml` |

---

## 3. 文件格式设计

### 3.1 Markdown + Frontmatter 格式

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

### 3.2 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Job 名称（同时作为唯一 ID） |
| `cron` | string | 是 | Cron 表达式 |
| `channel` | string | 否 | 目标渠道（telegram/discord 等） |
| `to` | string | 否 | 目标用户/聊天 ID |
| `enabled` | bool | 否 | 是否启用，默认 `true` |
| Body | string | 是 | 触发时发送的消息内容 |

### 3.3 文件命名约定

- 文件名（不含 `.md` 后缀）作为 job 的唯一标识符
- 例如：`morning-weather.md` → job ID = `morning-weather`

---

## 4. 核心行为设计

### 4.1 Gateway 启动流程

```rust
CronService::new(workspace)
├─ 扫描 ~/.gasket/cron/*.md
├─ 解析每个文件 → CronJob
├─ 计算 next_run = calculate_next_run(cron_expr)
├─ 存入内存 HashMap<id, InMemoryJob>
└─ 启动 notify 文件监听
```

### 4.2 轮询执行流程

```rust
CronChecker::tick() // 每 60s
├─ 遍历内存 jobs
├─ if job.enabled && job.next_run <= now:
│   ├─ publish_inbound(job.message)
│   └─ job.next_run = calculate_next_run(job.cron)
└─ 返回 due_jobs
```

### 4.3 启动时补偿策略 (A + C1)

```rust
fn on_startup(job: &CronJob) -> bool {
    let next_run = calculate_next_run(&job.cron);
    if next_run <= Utc::now() {
        // 到期立即执行一次
        return true; // should_execute_now
    }
    false
}
```

### 4.4 文件热重载

```rust
notify::watcher().with(move |event| {
    match event.kind {
        Modified(path) => {
            // 重新解析文件，更新内存 job
            reload_job(path)
        }
        Created(path) => {
            // 添加新 job
            add_job(path)
        }
        Removed(_) => {
            // 从内存中移除
            remove_job(path)
        }
    }
});
```

---

## 5. 迁移计划

### 5.1 现有数据处理

1. **YAML → Markdown** - 提供迁移脚本，将现有 `cron/*.yaml` 转换为 `cron/*.md`
2. **SQLite 清理** - 启动时检测 `cron_jobs` 表，打印警告（不再使用）

### 5.2 迁移脚本示例

```bash
# 迁移现有 YAML 文件
for f in ~/.gasket/cron/*.yaml; do
    name=$(basename "$f" .yaml)
    cat > "~/.gasket/cron/$name.md" <<EOF
---
$(cat "$f")
---

EOF
done
```

---

## 6. API 变更

### 6.1 移除的 API

| 模块 | 移除项 | 理由 |
|------|--------|------|
| `storage/src/cron.rs` | `CronJobRow` | 不再使用 SQLite |
| `storage/src/cron.rs` | `save_cron_job()` | 不再持久化到 DB |
| `storage/src/cron.rs` | `load_cron_jobs()` | 直接从文件加载 |
| `storage/src/cron.rs` | `load_due_cron_jobs()` | 内存中过滤 |
| `engine/src/cron/service.rs` | `mark_job_run()` | 无需更新 DB |

### 6.2 保留的 API

| 模块 | 保留项 | 说明 |
|------|--------|------|
| `engine/src/cron/service.rs` | `CronService` | 重构为内存存储 |
| `engine/src/cron/service.rs` | `list_jobs()` | 返回内存 jobs |
| `engine/src/cron/service.rs` | `get_due_jobs()` | 内存过滤 |
| `engine/src/tools/cron.rs` | `CronTool` | CLI 工具保留 |

### 6.3 CronTool 行为变更

- `add` 动作：创建 `.md` 文件（而非写入 DB）
- `remove` 动作：删除 `.md` 文件
- `list` 动作：扫描 `cron/*.md` 文件

---

## 7. 错误处理

### 7.1 解析错误

| 场景 | 处理 |
|------|------|
| frontmatter 格式错误 | 记录警告，跳过该文件 |
| cron 表达式无效 | 记录错误，job 标记为 disabled |
| 缺少必填字段 | 记录警告，跳过该文件 |

### 7.2 文件监听错误

| 场景 | 处理 |
|------|------|
| cron 目录不存在 | 创建目录，继续运行 |
| notify 初始化失败 | 降级为启动时一次性加载 |

---

## 8. 测试策略

### 8.1 单元测试

- [ ] `parse_markdown()` 解析 frontmatter
- [ ] `calculate_next_run()` cron 表达式解析
- [ ] `should_execute_now()` 启动补偿逻辑

### 8.2 集成测试

- [ ] 文件热重载：修改 `.md` 文件后 60s 内生效
- [ ] 启动时到期任务立即执行
- [ ] CLI `add`/`remove`/`list` 操作文件

---

## 9. 依赖变更

### 9.1 新增依赖

```toml
[dependencies]
notify = "6"          # 文件监听
frontmatter = "0.9"   # frontmatter 解析（或使用 serde_yaml）
```

### 9.2 移除依赖

无（`cron` crate 保留，`sqlx` 仍被其他模块使用）

---

## 10. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 重启后丢失 `last_run` | 无法追踪历史执行 | 未来可扩展执行日志功能 |
| 文件监听延迟 | 修改后最多 60s 生效 | 文档说明，或支持手动 reload 命令 |
| 并发文件修改 | 可能丢失更新 | 文档建议单次修改一个文件 |

---

## 11. 验收标准

1. [ ] Gateway 启动后，`cron/*.md` 文件被正确加载
2. [ ] 到期的 cron job 在启动时立即执行一次
3. [ ] 修改 `enabled: false` 后 60s 内 job 停止执行
4. [ ] CLI `cron add` 创建 `.md` 文件
5. [ ] CLI `cron remove` 删除 `.md` 文件
6. [ ] 移除所有 SQLite cron 相关代码
7. [ ] 所有测试通过

---

## 附录 A：示例文件

### A.1 简单定时任务

```markdown
---
name: daily-standup
cron: "0 9 * * *"
channel: slack
to: "C12345678"
enabled: true
---

请提醒团队成员提交每日站会更新
```

### A.2 高频任务

```markdown
---
name: health-check
cron: "*/5 * * * *"
channel: telegram
to: "123456789"
enabled: true
---

检查系统健康状态并报告
```

### A.3 禁用任务

```markdown
---
name: legacy-report
cron: "0 0 * * *"
enabled: false
---

此任务已废弃
```
