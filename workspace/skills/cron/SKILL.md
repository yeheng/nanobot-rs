---
name: cron
description: Schedule and manage recurring tasks using file-driven cron system
always: true
---

# Cron Task Management

Manage recurring tasks using Markdown + YAML frontmatter. Requires manual refresh after changes.

## Quick Start

### Create a Task

```markdown
---
name: daily-standup
cron: "0 9 * * 1-5"
channel: telegram
to: "group_chat_123"
enabled: true
---

Good morning! Please submit your daily standup update.
```

### Management Commands

```bash
gasket cron list          # List all tasks
gasket cron show <id>     # Show task details
gasket cron remove <id>   # Remove a task
gasket cron enable <id>   # Enable a task
gasket cron disable <id>  # Disable a task
gasket cron refresh       # Manually refresh all cron tasks
```

## Cron Expression

Standard 5-field format is recommended: `Minute Hour Day Month Weekday`

```
┌───────────── Minute (0-59)
│ ┌───────────── Hour (0-23)
│ │ ┌───────────── Day (1-31)
│ │ │ ┌───────────── Month (1-12)
│ │ │ │ ┌───────────── Weekday (0-6, 0=Sunday)
* * * * *
```

6-field (`Sec Min Hour Day Month Weekday`) and 7-field (`Sec Min Hour Day Month Weekday Year`) are also accepted.

**Common Patterns:**
- `0 9 * * *` - Every day at 9:00
- `0 9 * * 1-5` - Weekdays at 9:00
- `*/30 * * * *` - Every 30 minutes
- `0 */2 * * *` - Every 2 hours

## Task Configuration

| Field | Required | Type | Default | Description |
|-------|----------|------|---------|-------------|
| `name` | No | string | filename | Task name |
| `cron` | Yes | string | - | Cron expression |
| `channel` | No | string | `websocket` | Channel (telegram/discord/slack/websocket). If not specified, broadcasts to all WebSocket clients |
| `to` | No | string | - | Target chat ID |
| `enabled` | No | boolean | `true` | Enable status |

**Important:** The message content goes in the **markdown body** (after `---`), NOT in the YAML frontmatter header. Do NOT add `message` field to the YAML frontmatter.

| Body | Yes | string | - | Message to send when triggered |

## Manual Refresh Required

Cron tasks are stored in `~/.gasket/cron/*.md`. **Changes do NOT auto-refresh** - you must manually reload:

- After adding/modifying/removing task files → run `gasket cron refresh` or restart gateway
- The gateway loads tasks on startup
- **DO NOT write markdown files directly to the cron directory** - always use the CLI tool or MCP tool to add/remove cron tasks

## Best Practices

1. Use descriptive names
2. Test cron expressions first
3. Route to the correct channel
4. Clean up old tasks regularly
5. Be aware of server timezone (default UTC)

## Troubleshooting

**Task not running:** Check enabled status, cron expression, channel configuration, gateway logs  
**Wrong time:** Confirm server timezone, verify cron expression  
**Changes not applied:** Run `gasket cron refresh` to reload all tasks, or restart the gateway

## Important Notes

- Requires gateway to be running
- Executes due tasks on startup
- Recommended limit: 100 tasks maximum
- **Always use CLI commands or tools to manage cron tasks, not direct file manipulation**
