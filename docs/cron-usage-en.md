# Cron Job Usage Guide

> How to use scheduled tasks in Gasket

---

## Defining Cron Jobs

### Method 1: HEARTBEAT.md File

Create or edit `~/.gasket/HEARTBEAT.md`:

```markdown
## Daily Summary
- cron: 0 9 * * *
- message: Generate daily work summary

## Weekly Report
- cron: 0 10 * * 1
- message: Create weekly progress report

## Health Check
- cron: */30 * * * *
- message: Check system health status
```

### Method 2: Via CLI

```bash
# Add a new cron job
gasket cron add "daily-report" "0 9 * * *" "Generate daily report"

# List all jobs
gasket cron list

# Remove a job
gasket cron remove "daily-report"
```

### Method 3: Via Tool Call

```
You: Remind me every morning at 8am

🤖 Gasket uses the cron tool:
```json
{
  "action": "create",
  "name": "morning-reminder",
  "cron": "0 8 * * *",
  "message": "Good morning! Time to plan your day."
}
```
```

---

## Cron Expression Format

```
┌───────────── minute (0 - 59)
│ ┌───────────── hour (0 - 23)
│ │ ┌───────────── day of month (1 - 31)
│ │ │ ┌───────────── month (1 - 12)
│ │ │ │ ┌───────────── day of week (0 - 6, Sunday = 0)
│ │ │ │ │
│ │ │ │ │
* * * * *
```

### Common Patterns

| Expression | Description |
|------------|-------------|
| `0 9 * * *` | Every day at 9:00 AM |
| `0 */6 * * *` | Every 6 hours |
| `0 9 * * 1` | Every Monday at 9:00 AM |
| `0 9 1 * *` | 1st of every month at 9:00 AM |
| `*/5 * * * *` | Every 5 minutes |
| `0 9-17 * * 1-5` | Every hour 9-17 on weekdays |

---

## Job Parameters

```markdown
## Job Name
- cron: expression
- message: Message sent to AI when triggered
- channel: telegram/discord/slack (optional)
- chat_id: Target chat ID (optional)
```

---

## Managing Cron Jobs

### CLI Commands

```bash
# List all jobs
gasket cron list

# Enable/disable a job
gasket cron enable "job-name"
gasket cron disable "job-name"

# Delete a job
gasket cron delete "job-name"

# Trigger a job manually
gasket cron trigger "job-name"
```

### Viewing Job Status

```bash
# Show next run time for all jobs
gasket cron status

# Show job execution history
gasket cron history "job-name"
```

---

## Best Practices

1. **Idempotent Jobs**: Design jobs to be safe if run multiple times
2. **Error Handling**: Jobs should handle failures gracefully
3. **Reasonable Intervals**: Don't schedule too many jobs at the exact same time
4. **Descriptive Names**: Use clear job names for easier management

---

## Troubleshooting

### Job not running

- Check if cron service is enabled: `gasket cron status`
- Verify cron expression is valid
- Check logs for errors: `RUST_LOG=debug gasket gateway`

### Job running multiple times

- Ensure job names are unique
- Check if multiple Gasket instances are running

### Time zone issues

Cron jobs use the system time zone. To use a different timezone:

```bash
# Set timezone for Gasket
TZ=America/New_York gasket gateway
```
