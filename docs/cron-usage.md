# Cron Job Usage Guide

## Overview

Gasket uses a **file-driven architecture** for cron jobs. All job definitions are stored as Markdown files in `~/.gasket/cron/`. The service loads jobs from these files at startup and watches for changes via hot reload.

**Key Features:**
- No SQLite persistence — files are the Single Source of Truth (SSOT)
- Hot reload: edit a `.md` file and changes take effect within ~50ms
- Supports 6-field cron expressions (`sec min hour day month weekday`)
- Enabled/disabled state is stored in the file

## File Format

Each cron job is a Markdown file with YAML frontmatter:

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

### Frontmatter Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | No | (filename) | Human-readable job name |
| `cron` | string | **Yes** | - | Cron expression (6 fields: sec min hour day month weekday) |
| `channel` | string | No | (context default) | Target channel: `websocket`, `telegram`, `discord`, `cli`, etc. |
| `to` | string | No | (context default) | Target user/chat ID. For WebSocket, this is the `user_id` query parameter |
| `enabled` | boolean | No | `true` | Whether the job is active |

**Supported Channels:**
- `websocket` - WebSocket clients (default for gateway)
- `telegram` - Telegram bot
- `discord` - Discord bot
- `cli` - Command-line interface
- Custom channels as configured

**Note:** If `channel` and `to` are not specified, the job will use the channel context from when it was created (for API/tool usage) or broadcast to all connected WebSocket clients (for file-based jobs).

**Important:** The message content goes in the **markdown body** (after `---`), NOT in the YAML frontmatter header. Do NOT add `message` field to the YAML frontmatter.

### Body

The content after the frontmatter (`---` delimiter) is the **message** or **prompt** that will be sent when the job executes.

## CLI Commands

### List All Jobs

```bash
gasket cron list
```

Output:
```
Scheduled Jobs

morning-weather
  ID:       morning-weather
  Status:   ✓
  Cron:     */10 * * * *
  Message:  请获取未来三天广州天气情况并发送给用户
  Next:     2026-04-07 15:00 UTC
  Channel:  telegram
  Chat ID:  8281248569
```

### Add a New Job

```bash
gasket cron add "Job Name" "0 9 * * *" "Message content"
```

Or via agent:
```
请创建一个 cron 任务，每天早上 9 点发送日报提醒
```

**Cron Expression Format:**
- 6-field: `0 0 9 * * *` (sec min hour day month weekday)
- The CLI also accepts 5-field expressions and auto-converts them by prepending `0` for seconds

### Show Job Details

```bash
gasket cron show morning-weather
```

Shows detailed info including the next 5 scheduled run times.

### Enable/Disable a Job

```bash
gasket cron enable <job-id>
gasket cron disable <job-id>
```

### Remove a Job

```bash
gasket cron remove <job-id>
```

## Examples

### Example 1: WebSocket Reminder

Create a cron job that sends a reminder to a specific WebSocket user every hour:

```markdown
---
name: hourly-reminder
cron: "0 0 * * * *"
channel: websocket
to: user-123
enabled: true
---

记得休息一下，起来活动活动！
```

### Example 2: Telegram Daily Report

Send a daily report to a Telegram chat every morning at 9 AM:

```markdown
---
name: daily-report
cron: "0 0 9 * * *"
channel: telegram
to: "8281248569"
enabled: true
---

请生成今日工作总结
```

### Example 3: Direct Tool Execution

Execute a system maintenance tool without using LLM (zero token cost):

```markdown
---
name: memory-decay
cron: "0 0 */6 * * * *"
channel: websocket
to: admin
enabled: true
tool: memory_decay
---
```

### Example 4: Broadcast to All WebSocket Clients

Omit the `to` field to broadcast to all connected WebSocket clients:

```markdown
---
name: system-announcement
cron: "0 0 12 * * *"
channel: websocket
enabled: true
---

系统即将进行维护，请保存您的工作。
```

## Via Agent (Tool Usage)

You can also create cron jobs through the agent using the `cron` tool:

```
请创建一个 cron 任务，每小时通过 websocket 给用户 test-user 发送提醒
```

The agent will call the `cron` tool with:
```json
{
  "action": "add",
  "name": "Hourly Reminder",
  "cron": "0 0 * * * *",
  "message": "记得休息一下",
  "channel": "websocket",
  "to": "test-user"
}
```

## Hot Reload

The cron service watches the `~/.gasket/cron/` directory for file changes:

- **Modify**: Edit a `.md` file → job is reloaded within ~50ms
- **Create**: Add a new `.md` file → job is loaded automatically
- **Delete**: Remove a `.md` file → job is removed from memory

No restart required.

## File Location

```
~/.gasket/cron/
├── morning-weather.md
├── daily-report.md
└── weekly-backup.md
```

## Manual File Editing

You can directly edit the `.md` files to update jobs. For example:

```bash
# Edit the cron schedule
vim ~/.gasket/cron/morning-weather.md

# Change from:
# cron: "*/10 * * * *"
# to:
# cron: "0 8 * * *"
```

The change takes effect immediately via hot reload.

## Architecture Notes

**In-Memory State:**
- Jobs are loaded into a `HashMap<String, CronJob>` at startup
- `next_run` is calculated in-memory using the `cron` crate (v0.15)
- File watcher uses `notify` crate (v7) for cross-platform file monitoring

**Startup Behavior:**
- If a job's scheduled time has passed while the service was down, it will be executed immediately on startup
- This ensures no scheduled jobs are missed during downtime

**Thread Safety:**
- `parking_lot::RwLock` for concurrent job access
- `Mutex<Receiver>` for file watcher events (required for `Send` across threads)
