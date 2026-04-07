---
name: cron
description: Schedule and manage recurring tasks using file-driven cron system
always: false
---

# Cron Task Management

使用 Markdown + YAML frontmatter 管理定时任务，支持热重载。

## 快速开始

### 创建任务

```markdown
---
name: daily-standup
cron: "0 0 9 * * 1-5"
channel: telegram
to: "group_chat_123"
enabled: true
---

各位早上好！请提交今日站会更新。
```

### 管理命令

```bash
gasket cron list          # 查看所有任务
gasket cron show <id>     # 查看任务详情
gasket cron remove <id>   # 删除任务
gasket cron enable <id>   # 启用任务
gasket cron disable <id>  # 禁用任务
```

## Cron 表达式

6 位格式：`秒 分 时 日 月 周`

```
┌───────────── 秒 (0-59)
│ ┌───────────── 分 (0-59)
│ │ ┌───────────── 时 (0-23)
│ │ │ ┌───────────── 日 (1-31)
│ │ │ │ ┌───────────── 月 (1-12)
│ │ │ │ │ ┌───────────── 周 (0-6, 0=周日)
* * * * * *
```

**常用模式：**
- `0 0 9 * * *` - 每天 9:00
- `0 0 9 * * 1-5` - 工作日 9:00
- `0 0 */2 * * *` - 每 2 小时
- `*/30 * * * * *` - 每 30 秒

## 任务配置

| 字段 | 必填 | 类型 | 默认 | 说明 |
|------|------|------|------|------|
| `name` | 否 | string | 文件名 | 任务名称 |
| `cron` | 是 | string | - | Cron 表达式 |
| `channel` | 否 | string | - | 渠道 (telegram/discord/slack) |
| `to` | 否 | string | - | 目标聊天 ID |
| `enabled` | 否 | boolean | `true` | 启用状态 |
| Body | 是 | string | - | 触发时发送的消息 |

## 热重载

文件存储于 `~/.gasket/cron/*.md`，修改后 50-100ms 自动生效：
- 新建/修改文件 → 立即加载
- 删除文件 → 立即移除
- 重启 gateway → 强制重载

## 最佳实践

1. 使用描述性名称
2. 先测试 cron 表达式
3. 路由到正确渠道
4. 定期清理旧任务
5. 注意服务器时区（默认 UTC）

## 故障排查

**任务未运行：** 检查 enabled 状态、cron 表达式、渠道配置、gateway 日志  
**时间不对：** 确认服务器时区、验证 cron 表达式  
**修改未生效：** 确认文件已保存、扩展名为.md、等待 60 秒或重启 gateway

## 注意事项

- 需要 gateway 运行
- 启动时会立即执行到期任务
- 建议不超过 100 个任务
