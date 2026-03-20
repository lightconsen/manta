---
name: cron
description: "Schedule and manage recurring tasks with cron-like expressions"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "cron"
    priority: 100
  - type: keyword
    pattern: "schedule"
    priority: 80
  - type: keyword
    pattern: "recurring"
    priority: 70
openclaw:
  emoji: "⏰"
  category: "automation"
  tags:
    - "cron"
    - "schedule"
    - "automation"
---

# Cron Skill

Schedule and manage recurring tasks using cron expressions.

## Capabilities

- Create scheduled jobs with standard cron expressions
- List, enable, disable, and delete scheduled tasks
- View upcoming scheduled runs
- Execute one-off delayed tasks

## Usage Examples

### Schedule a task
Tell the agent: "Schedule a daily summary at 9am" or "Run health check every 5 minutes"

### List scheduled jobs
"Show me all scheduled tasks" or "What cron jobs are active?"

### Cron Expression Format

```
┌─────── minute (0-59)
│ ┌───── hour (0-23)
│ │ ┌─── day of month (1-31)
│ │ │ ┌─ month (1-12)
│ │ │ │ ┌ day of week (0-6, Sun=0)
│ │ │ │ │
* * * * *
```

### Common Patterns

- `0 9 * * 1-5` — Weekdays at 9am
- `*/5 * * * *` — Every 5 minutes
- `0 0 * * *` — Daily at midnight
- `0 */2 * * *` — Every 2 hours

## Best Practices

1. Use descriptive names for scheduled tasks
2. Log task outcomes for debugging
3. Handle failures gracefully with retry logic
4. Avoid scheduling too many high-frequency tasks
