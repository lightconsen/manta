# Reminder Skill

Set reminders for future events or tasks.

## Triggers

- Regex: `remind me (?:to|about)\s+(.+?)(?:\s+(?:in|at|on)\s+(.+))?$`
- Regex: `set (?:a )?reminder`
- Keyword: "remind"
- Intent: "set_reminder"

## Prompt

When the user wants to set a reminder, use the `cron` tool to schedule a future notification.

Parse the natural language time expression and convert it to a cron expression or use the natural language scheduling.

## Example Usage

**User:** "Remind me to call mom in 30 minutes"

**Action:** Schedule a reminder for 30 minutes from now

**Response:**
```
⏰ Reminder set!
"Call mom" in 30 minutes (at 3:45 PM)
```

**User:** "Remind me about the meeting tomorrow at 9am"

**Action:** Schedule for tomorrow at 9:00 AM

**Response:**
```
⏰ Reminder set!
"Meeting" tomorrow at 9:00 AM
```

**User:** "Remind me to take medicine every day at 8pm"

**Action:** Create a recurring daily reminder

**Response:**
```
⏰ Recurring reminder set!
"Take medicine" every day at 8:00 PM
```

## Time Expressions Supported

- Relative: "in 30 minutes", "in 2 hours", "in 3 days"
- Absolute: "at 3pm", "at 15:30"
- Dates: "tomorrow at 9am", "next Monday", "Dec 25 at 8am"
- Recurring: "every day at 8pm", "every Monday", "every hour"

## Configuration

```yaml
notification_method: "message"  # or "email", "push"
default_reminder_time: "9:00 AM"
timezone: "America/New_York"
```

## Notes

- Uses the cron scheduler for recurring reminders
- One-time reminders are cleaned up after triggering
- Supports snooze functionality
- Timezone-aware scheduling
