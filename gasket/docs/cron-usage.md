# Cron 使用指南

> 文件驱动的定时任务系统

---

## 概述

Gasket 的 Cron 系统采用**文件驱动架构**，所有任务定义存储在 `~/.gasket/cron/*.md` 文件中。

**核心特点：**
- **Markdown + YAML Frontmatter** 格式，易于编辑和版本控制
- **热重载支持** - 修改文件后自动加载，无需重启 gateway
- **内存状态管理** - 运行时状态（next_run）在内存中计算，无需数据库
- **CLI 工具集成** - 支持通过 CLI 或自然语言添加/删除任务

---

## 文件格式

### 基本结构

```markdown
---
name: 任务名称
cron: "0 9 * * *"
channel: telegram
to: "chat_id"
enabled: true
---

任务触发时发送的消息内容
```

### 字段说明

| 字段 | 必填 | 类型 | 默认值 | 说明 |
|------|------|------|--------|------|
| `name` | 否 | string | 文件名 | 任务名称 |
| `cron` | 是 | string | - | Cron 表达式（6 字段格式，含秒） |
| `channel` | 否 | string | - | 目标渠道（telegram/discord/slack 等） |
| `to` | 否 | string | - | 目标聊天/用户 ID |
| `enabled` | 否 | boolean | `true` | 是否启用任务 |
| Body | 是 | string | - | 任务触发时的消息内容 |

### 文件命名

- 文件位于 `~/.gasket/cron/` 目录
- 使用 `.md` 扩展名
- 文件名（不含扩展名）作为任务的唯一 ID

---

## Cron 表达式格式

### 6 字段格式（含秒）

```
┌───────────── 秒 (0 - 59)
│ ┌───────────── 分钟 (0 - 59)
│ │ ┌───────────── 小时 (0 - 23)
│ │ │ ┌───────────── 日期 (1 - 31)
│ │ │ │ ┌───────────── 月份 (1 - 12)
│ │ │ │ │ ┌───────────── 星期 (0 - 6, 0=周日)
│ │ │ │ │ │
* * * * * *
```

### 常用示例

| 表达式 | 说明 |
|--------|------|
| `0 0 9 * * *` | 每天早上 9:00 |
| `0 30 14 * * *` | 每天下午 2:30 |
| `0 0 9 * * 1-5` | 工作日早上 9:00 |
| `0 0 */2 * * *` | 每 2 小时 |
| `0 0 10 * * 0,6` | 周末上午 10:00 |
| `30 8 1 * * *` | 每月 1 号上午 8:00:30 |

### 特殊语法

| 语法 | 说明 |
|------|------|
| `*` | 每个值 |
| `*/5` | 每 5 个单位 |
| `1-5` | 范围 1 到 5 |
| `1,3,5` | 枚举值 |
| `1-5/2` | 范围内每 2 个单位 |

---

## 使用示例

### 示例 1：每日站会提醒

```markdown
---
name: daily-standup
cron: "0 0 9 * * *"
channel: telegram
to: "group_chat_123"
enabled: true
---

各位早上好！请提交今日的站会更新：
1. 昨天完成了什么？
2. 今天计划做什么？
3. 有什么阻碍需要帮助？
```

### 示例 2：系统健康检查

```markdown
---
name: health-check
cron: "0 0 */2 * * *"
channel: slack
to: "ops_channel"
enabled: true
---

请检查系统健康状态：
- API 响应时间
- 数据库连接
- 磁盘使用率
- 错误日志数量
```

### 示例 3：周报提醒

```markdown
---
name: weekly-report
cron: "0 0 17 * * 5"
channel: feishu
to: "team_group"
enabled: true
---

本周工作即将结束，请提交周报！
截止时间：今天下午 6 点
```

### 示例 4：禁用的任务

```markdown
---
name: legacy-task
cron: "0 0 0 * * *"
enabled: false
---

此任务已废弃，保留文件作为参考
```

---

## CLI 命令

### 列出所有任务

```bash
gasket cron list
```

### 添加任务

```bash
gasket cron add "任务名称" "cron 表达式" "消息内容"
```

示例：
```bash
gasket cron add "morning-reminder" "0 0 9 * * *" "早上好！记得提交站会更新"
```

### 删除任务

```bash
gasket cron remove <task-id>
```

### 查看任务详情

```bash
gasket cron show <task-id>
```

### 启用/禁用任务

```bash
gasket cron enable <task-id>
gasket cron disable <task-id>
```

---

## 热重载

Cron 服务使用 `notify` crate 监听文件变化：

| 事件 | 行为 |
|------|------|
| 文件修改 | 重新解析并更新内存中的任务 |
| 文件创建 | 加载新任务 |
| 文件删除 | 从内存中移除任务 |

**注意：** 文件变更后通常在 50-100ms 内生效。

---

## 最佳实践

1. **使用描述性名称** - 便于识别和管理
2. **测试 Cron 表达式** - 使用在线工具验证表达式正确性
3. **合理设置渠道** - 将提醒发送到合适的聊天
4. **定期审查** - 清理不再需要的任务
5. **注意时区** - 服务器使用 UTC 时间

---

## 故障排查

### 任务未执行

1. 检查任务是否启用（`enabled: true`）
2. 验证 Cron 表达式是否正确
3. 确认渠道配置正确
4. 查看 gateway 日志

### 时间不正确

1. 检查服务器时区设置
2. 验证 Cron 表达式
3. 考虑使用明确的时间（如 `0 0 9 * * *` 表示 9:00）

### 文件修改未生效

1. 确认文件已保存
2. 检查文件名是否为 `.md` 扩展名
3. 等待最多 60 秒让 watcher 检测到变更
4. 重启 gateway 强制重新加载

---

## 技术细节

### 启动时行为

1. 扫描 `~/.gasket/cron/*.md` 文件
2. 解析每个文件的 frontmatter 和 body
3. 计算每个任务的 `next_run` 时间
4. 启动文件监听器

### 执行流程

```
CronChecker (每 60s)
  ↓
遍历内存中的任务
  ↓
if enabled && next_run <= now:
  ├─ publish_inbound(message)
  └─ update_next_run()
```

### 启动补偿

如果任务在启动时已到期，会立即执行一次。

---

## 相关文件

- 设计文档：`docs/superpowers/specs/2026-04-07-cron-file-driven-design.md`
- 实现计划：`docs/superpowers/plans/2026-04-07-cron-file-driven-implementation.md`
- 服务实现：`engine/src/cron/service.rs`
- CLI 实现：`cli/src/commands/cron.rs`
- Tool 实现：`engine/src/tools/cron.rs`
