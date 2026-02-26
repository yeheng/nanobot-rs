# Tool Usage Notes

Tool signatures are provided via function definitions. This document covers **supplementary guidance only**.

## exec Safety

- Commands have a configurable timeout (default 60s)
- Dangerous commands are blocked (rm -rf, format, dd, shutdown, etc.)
- Output is truncated at 10,000 characters

## Scheduled Reminders (Cron)

Use `exec` to create reminders with `nanobot cron add`:

```bash
# Recurring
nanobot cron add --name "morning" --message "Good morning!" --cron "0 9 * * *"
nanobot cron add --name "water" --message "Drink water!" --every 7200

# One-time
nanobot cron add --name "meeting" --message "Meeting starts!" --at "2025-01-31T15:00:00"

# Manage
nanobot cron list
nanobot cron remove <job_id>
```

## Heartbeat Tasks

`HEARTBEAT.md` is checked every 30 minutes. Edit it with file tools to manage periodic tasks.

Task format: `- [ ] Description of periodic task`
