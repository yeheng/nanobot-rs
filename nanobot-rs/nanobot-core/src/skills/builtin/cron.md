---
name: cron
description: Schedule and manage recurring tasks and reminders
always: false
---

# Cron Task Management Skill

This skill provides guidance on managing scheduled tasks using nanobot's cron system.

## Overview

The cron system allows you to:
- Schedule recurring tasks (e.g., daily reports)
- Set up reminders (e.g., weekly meetings)
- Run one-time tasks at a specific time

## Cron Expression Format

Standard 5-field cron format:

```
┌───────────── minute (0 - 59)
│ ┌───────────── hour (0 - 23)
│ │ ┌───────────── day of month (1 - 31)
│ │ │ ┌───────────── month (1 - 12)
│ │ │ │ ┌───────────── day of week (0 - 6) (Sunday = 0)
│ │ │ │ │
* * * * *
```

## Common Patterns

| Expression | Description |
|------------|-------------|
| `0 9 * * *` | Every day at 9:00 AM |
| `0 9 * * 1` | Every Monday at 9:00 AM |
| `0 9 1 * *` | Every 1st of the month at 9:00 AM |
| `*/15 * * * *` | Every 15 minutes |
| `0 */2 * * *` | Every 2 hours |
| `0 9-17 * * *` | Every hour from 9 AM to 5 PM |

## Using the Cron Tool

### Add a Scheduled Task

```
Use the cron tool with action "add":

{
  "action": "add",
  "name": "daily-standup",
  "schedule": "0 9 * * 1-5",  // Weekdays at 9 AM
  "message": "Time for daily standup meeting!",
  "channel": "telegram",
  "chat_id": "123456"
}
```

### List All Tasks

```
Use the cron tool with action "list":

{
  "action": "list"
}

Returns:
- Task ID
- Task name
- Schedule
- Next run time
- Status (enabled/disabled)
```

### Remove a Task

```
Use the cron tool with action "remove":

{
  "action": "remove",
  "id": "task-uuid-here"
}
```

### Enable/Disable a Task

```
Use the cron tool with action "enable" or "disable":

{
  "action": "enable",
  "id": "task-uuid-here"
}
```

### Run a Task Manually

```
Use the cron tool with action "run":

{
  "action": "run",
  "id": "task-uuid-here"
}
```

## One-Time Tasks

For tasks that run once at a specific time:

```
{
  "action": "add",
  "name": "meeting-reminder",
  "at": "2024-01-20 14:30",  // Use "at" instead of "schedule"
  "message": "Meeting in 30 minutes",
  "channel": "telegram",
  "chat_id": "123456"
}
```

## Task Persistence

Tasks are persisted to `~/.nanobot/cron/jobs.json` and will survive restarts.

## Use Cases

### 1. Daily Standup Reminder
```
Add cron task:
- Name: "daily-standup"
- Schedule: "0 9 * * 1-5" (weekdays at 9 AM)
- Message: "Time for daily standup!"
- Channel: telegram
- Chat ID: 123456
```

### 2. Weekly Report
```
Add cron task:
- Name: "weekly-report"
- Schedule: "0 17 * * 5" (Friday at 5 PM)
- Message: "Don't forget to submit your weekly report!"
- Channel: slack
- Chat ID: general
```

### 3. Hourly Health Check
```
Add cron task:
- Name: "health-check"
- Schedule: "0 * * * *" (every hour)
- Message: "check_system_health" (custom action)
```

### 4. Monthly Backup Reminder
```
Add cron task:
- Name: "monthly-backup"
- Schedule: "0 2 1 * *" (1st of month at 2 AM)
- Message: "Time to perform monthly backup"
```

## Best Practices

1. **Use Meaningful Names**: Give tasks descriptive names for easy identification
2. **Test Schedules**: Verify cron expressions before deploying
3. **Set Appropriate Channels**: Route reminders to the right chat
4. **Review Regularly**: Periodically check and clean up old tasks
5. **Handle Timezones**: Be aware of the server's timezone setting

## Troubleshooting

### Task Not Running
- Check if task is enabled
- Verify cron expression is correct
- Ensure channel is configured and running
- Check logs for errors

### Wrong Time
- Check server timezone
- Verify cron expression
- Consider using explicit time (e.g., "0 9 * * *" for 9 AM UTC)

## CLI Commands

Users can also manage tasks via CLI:

```bash
# List all tasks
nanobot cron list

# Add a task
nanobot cron add --name "daily-reminder" --schedule "0 9 * * *" --message "Good morning!"

# Remove a task
nanobot cron remove --id <task-id>

# Enable/disable
nanobot cron enable --id <task-id>
nanobot cron disable --id <task-id>

# Run manually
nanobot cron run --id <task-id>
```

## Important Notes

- Tasks require the gateway to be running
- Task execution is best-effort (no guaranteed delivery)
- Timezone is determined by the server's system clock
- Maximum of 100 scheduled tasks (configurable)
