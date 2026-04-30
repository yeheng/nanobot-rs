---
name: cron
description: Schedule recurring tasks via Markdown + YAML cron files
always: false
---

# Cron

File-driven cron with SQLite state persistence. Tasks live in `~/.gasket/cron/*.md`.

## CLI

```bash
gasket cron add -n "Name" -c "0 9 * * 1-5" -m "Message"   # create
gasket cron list                                         # list all
gasket cron show <id>                                    # next 5 runs
gasket cron remove/enable/disable <id>                   # manage
gasket cron refresh                                      # reload from disk
```

## File Format

```markdown
---
name: daily-standup
cron: "0 9 * * 1-5"
channel: telegram        # target channel (optional, default: websocket)
to: "group_chat_123"     # chat/user ID (optional)
enabled: true
tool: weather_fetcher    # direct tool execution (optional, zero-token)
tool_args: { city: "Shanghai" }  # tool args (optional)
---
Message content here (LLM mode) or task description.
```

## Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `cron` | Yes | - | 5/6/7-field expression |
| `name` | No | filename | Human-readable name |
| `channel` | No | `websocket` | telegram/discord/slack/webhook/websocket |
| `to` | No | - | Target chat/user ID |
| `enabled` | No | `true` | Enable flag |
| `tool` | No | - | Direct tool name (bypasses LLM) |
| `tool_args` | No | - | JSON args for direct tool |

## Execution Modes

- **LLM mode** (default): message sent through agent loop, consumes tokens.
- **Direct tool mode** (`tool` field): executes tool directly, zero tokens.

## Notes

- Polling interval: 60s. Catches up missed ticks on startup.
- Changes require `gasket cron refresh` or gateway restart.
- Max recommended: 100 tasks.
