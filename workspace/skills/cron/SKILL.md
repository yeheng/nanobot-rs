---
name: cron
description: Schedule and manage recurring tasks using file-driven cron system
always: true
---

# Cron Task Management

Manage recurring tasks using Markdown + YAML frontmatter with SQLite state persistence. Supports both LLM-powered messages and direct tool execution.

## Quick Start

### Create a Task

**Option 1: Using CLI (Recommended)**
```bash
gasket cron add -n "Daily Standup" -c "0 9 * * 1-5" -m "Good morning! Please submit your daily standup update."
```

**Option 2: Markdown File**
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
gasket cron list          # List all scheduled jobs
gasket cron add           # Add a new job (interactive or with flags)
gasket cron show <id>     # Show task details and next 5 run times
gasket cron remove <id>   # Remove a task by ID
gasket cron enable <id>   # Enable a disabled task
gasket cron disable <id>  # Disable an enabled task
gasket cron refresh       # Manually reload all cron tasks from disk
```

### MCP Tool

Use the `cron` MCP tool with actions: `add`, `list`, `remove`, `refresh`

## Cron Expression

Supports 5-field (standard), 6-field, and 7-field formats. All expressions are normalized to 7-field internally.

**5-field (Recommended):** `Minute Hour Day Month Weekday`
```
┌───────────── Minute (0-59)
│ ┌───────────── Hour (0-23)
│ │ ┌───────────── Day (1-31)
│ │ │ ┌───────────── Month (1-12)
│ │ │ │ ┌───────────── Weekday (0-6, 0=Sunday)
* * * * *
```

**6-field:** `Second Minute Hour Day Month Weekday`  
**7-field:** `Second Minute Hour Day Month Weekday Year`

**Common Patterns:**
- `0 9 * * *` - Every day at 9:00
- `0 9 * * 1-5` - Weekdays at 9:00
- `*/30 * * * *` - Every 30 minutes
- `0 */2 * * *` - Every 2 hours at minute 0
- `0 0 * * *` - Every day at midnight

## Task Configuration

### YAML Frontmatter Fields

| Field | Required | Type | Default | Description |
|-------|----------|------|---------|-------------|
| `name` | No | string | filename | Human-readable task name |
| `cron` | **Yes** | string | - | Cron expression (5/6/7 fields) |
| `channel` | No | string | `websocket` | Target channel (telegram/discord/slack/webhook/websocket). If not specified, broadcasts to all WebSocket clients |
| `to` | No | string | - | Target chat/user ID |
| `enabled` | No | boolean | `true` | Enable status |
| `tool` | No | string | - | **Direct tool execution** - Tool name to execute (bypasses LLM, zero token cost) |
| `tool_args` | No | JSON object | - | JSON arguments for the tool |

### Message Content

**Important:** The message content goes in the **markdown body** (after `---`), NOT in the YAML frontmatter.

| Body | Yes | string | - | Message/trigger content when task runs |

### Execution Modes

**Mode A: LLM-Powered (Traditional)**
- No `tool` field defined
- Message sent through agent loop via message bus
- Agent processes and generates response
- Consumes tokens

**Mode B: Direct Tool Execution (Zero-Token)**
- Define `tool` field with tool name
- Define `tool_args` for parameters
- Executes tool directly without LLM
- Result/error sent to output channel
- **Example:**
```markdown
---
name: weather-alert
cron: "0 8 * * *"
tool: weather_fetcher
tool_args:
  city: "Shanghai"
  unit: "celsius"
---

Fetch and send morning weather forecast
```

## Architecture

**Storage:**
- **Configuration:** `~/.gasket/cron/*.md` - Markdown files with YAML frontmatter (Single Source of Truth)
- **State:** SQLite database at `~/.gasket/config_dir()` - Table `cron_state` stores `last_run_at` and `next_run_at`

**Scheduler:**
- Polling interval: **60 seconds**
- Runs as background task in gateway mode
- Handles missed ticks on startup (catches up on tasks that should have run while gateway was stopped)
- File change detection via mtime/size comparison

**Lifecycle:**
1. Gateway starts → loads all `.md` files from `~/.gasket/cron/`
2. Parses YAML frontmatter + markdown body
3. Restores state from SQLite (last_run, next_run)
4. Polls every 60s for due jobs
5. Executes via LLM or direct tool
6. Updates state in SQLite

## Manual Refresh Required

Cron tasks use file-based configuration with state persistence. **Changes do NOT auto-refresh**:

- After adding/modifying/removing task files → run `gasket cron refresh` or restart gateway
- The gateway loads tasks on startup and persists execution state to SQLite
- **Recommended:** Always use CLI commands or MCP tool to manage cron tasks, not direct file manipulation

## Best Practices

1. **Use descriptive names** - Makes identification easier in logs and lists
2. **Test cron expressions** - Use `gasket cron show` to verify next 5 run times
3. **Route correctly** - Specify `channel` and `to` for targeted messages
4. **Prefer direct tool execution** - Use `tool` field for zero-token automation
5. **Clean up old tasks** - Remove disabled tasks regularly
6. **Mind the timezone** - Server uses UTC by default
7. **Respect limits** - Recommended maximum: 100 tasks

## Troubleshooting

**Task not running:**
- Check `enabled` status in `gasket cron list`
- Verify cron expression format
- Confirm channel configuration exists
- Check gateway logs for errors
- Ensure gateway is running

**Wrong execution time:**
- Confirm server timezone (default UTC)
- Verify cron expression with `gasket cron show <id>`
- Check if task was missed during gateway downtime

**Changes not applied:**
- Run `gasket cron refresh` to reload all tasks
- Or restart the gateway
- Always use CLI/MCP tool instead of direct file edits

**Direct tool execution not working:**
- Verify `tool` name matches registered tool exactly
- Ensure `tool_args` is valid JSON format
- Check tool has required context/permissions

## Important Notes

- **Requires gateway** - Cron jobs only execute when gateway is running
- **Startup execution** - Tasks that missed during downtime can execute on startup if configured
- **State persistence** - Execution history (last_run, next_run) stored in SQLite
- **Recommended limit** - Maximum 100 tasks for optimal performance
- **File-based SSOT** - Markdown files are single source of truth, database is for state only
- **CLI-first approach** - Always use `gasket cron` commands or MCP tool for management
