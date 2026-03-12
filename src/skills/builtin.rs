//! Built-in skills for Manta
//!
//! These skills are always available and cannot be uninstalled.
//! They provide core functionality for skill management and scheduling.

use super::{Skill, SkillTrigger, TriggerType, StorageLevel};
use std::collections::HashMap;

/// Get all built-in skills
pub fn get_builtin_skills() -> HashMap<String, Skill> {
    let mut skills = HashMap::new();

    // Add each built-in skill
    let skill_creator = create_skill_creator();
    skills.insert(skill_creator.name.clone(), skill_creator);

    let find_skills = create_find_skills();
    skills.insert(find_skills.name.clone(), find_skills);

    let cron_skill = create_cron_skill();
    skills.insert(cron_skill.name.clone(), cron_skill);

    skills
}

/// Create the skill-creator built-in skill
fn create_skill_creator() -> Skill {
    let mut skill = Skill::new(
        "skill-creator",
        "Create and package new skills for Manta",
        SKILL_CREATOR_PROMPT,
    )
    .with_emoji("🛠️")
    .by("manta");

    // Add triggers
    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "skill-create".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "create skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "new skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "make skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    // Mark as built-in
    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the find-skills built-in skill
fn create_find_skills() -> Skill {
    let mut skill = Skill::new(
        "find-skills",
        "Search for and discover skills across all storage levels",
        FIND_SKILLS_PROMPT,
    )
    .with_emoji("🔍")
    .by("manta");

    // Add triggers
    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "find-skills".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "find skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "search skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "list skills".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "show skills".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    // Mark as built-in
    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the cron built-in skill
fn create_cron_skill() -> Skill {
    let mut skill = Skill::new(
        "cron",
        "Schedule recurring tasks and automated jobs",
        CRON_PROMPT,
    )
    .with_emoji("⏰")
    .by("manta");

    // Add triggers
    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "cron".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "schedule".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "recurring task".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "daily report".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "weekly summary".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "hourly check".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    // Mark as built-in
    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Skill creator prompt
const SKILL_CREATOR_PROMPT: &str = r#"# Skill Creator

Create new skills for Manta using the SKILL.md format.

## Quick Start

To create a new skill, guide the user through:

1. **Choose a name** - Short, lowercase, hyphenated (e.g., "weather", "github-pr")
2. **Write description** - What the skill does
3. **Define triggers** - Keywords or commands that activate it
4. **Write instructions** - Detailed guidance for the AI

## SKILL.md Format

```yaml
---
name: skill-name
description: "What this skill does"
version: "1.0.0"
triggers:
  - type: keyword
    pattern: "keyword"
  - type: command
    pattern: "command"
openclaw:
  emoji: "🛠️"
  requires:
    bins: ["optional-binary"]
    env: ["OPTIONAL_ENV_VAR"]
---

# Skill Instructions

Detailed instructions for the AI on how to use this skill...
```

## Storage Locations

Skills can be stored at different levels:
- **User**: `~/.config/manta/skills/` - Available everywhere
- **Project**: `./.manta/skills/` - Project-specific
- **Workspace**: Workspace root `.manta/skills/`

## Best Practices

1. **Specific triggers** - Use unique keywords to avoid conflicts
2. **Clear instructions** - Be explicit about what the AI should do
3. **Requirements** - Declare any binaries or env vars needed
4. **Examples** - Include example usage in the skill content
5. **Test it** - Try the skill after creating it

## Example Skills

### Simple Keyword Skill
```yaml
---
name: weather
description: "Get weather information"
triggers:
  - type: keyword
    pattern: "weather"
openclaw:
  emoji: "🌤️"
---

When the user asks about weather, use the `weather` tool or curl to fetch current conditions.
```

### Command Skill
```yaml
---
name: git-summary
description: "Summarize git activity"
triggers:
  - type: command
    pattern: "git-summary"
openclaw:
  emoji: "📊"
---

Run `git log --oneline --since="1 day ago"` and summarize the commits.
```

## Creation Steps

1. Create directory: `mkdir -p ~/.config/manta/skills/{name}`
2. Create SKILL.md: `touch ~/.config/manta/skills/{name}/SKILL.md`
3. Write the skill content
4. Test by triggering one of the patterns

Use `/find-skills` to verify the skill was loaded correctly.
"#;

/// Find skills prompt
const FIND_SKILLS_PROMPT: &str = r#"# Find Skills

Search for and discover skills across all storage levels in Manta.

## Usage

You can search for skills using:
- **Keywords** - Find skills matching specific keywords
- **Commands** - List all available slash commands
- **All skills** - Show every skill at every level

## Skill Storage Levels

Skills exist at multiple levels (highest priority first):

1. **Project** (`./.manta/skills/`) - Project-specific skills
2. **Workspace** - Workspace-level skills
3. **User** (`~/.config/manta/skills/`) - Your personal skills
4. **Bundled** - Built-in skills that come with Manta

## Search Commands

When the user asks to find skills, use the skill manager to:

1. **List all skills**: Show skills from all storage levels
2. **Find by keyword**: Search for skills matching specific terms
3. **Show eligible**: Only show skills that can run (requirements met)
4. **Show commands**: List all slash commands

## Output Format

Present results clearly:

```
Found X skills:

🛠️ skill-creator (built-in)
   Create and package new skills for Manta
   Trigger: /skill-create

🌤️ weather (user)
   Get weather information
   Trigger: "weather" keyword
   Requires: curl

❌ github (user) [not eligible]
   GitHub operations
   Missing: gh CLI not found
```

## Tips

- Highlight built-in skills (they're always available)
- Mark ineligible skills and explain why
- Show the trigger method for each skill
- Group by storage level when listing all
"#;

/// Cron skill prompt
const CRON_PROMPT: &str = r#"# Cron - Scheduled Tasks

Schedule recurring tasks and automated jobs in Manta.

## Features

- Schedule prompts to run automatically
- Natural language scheduling ("every day at 9am")
- Standard cron expressions ("0 9 * * *")
- View, enable, disable, and remove jobs

## Usage

### Natural Language Schedules

- "every hour" / "hourly"
- "every day" / "daily" / "every day at 9am"
- "every week" / "weekly"
- "every month" / "monthly"

### Cron Expressions

Standard 5-field cron: `minute hour day month weekday`

Examples:
- `0 * * * *` - Every hour
- `0 9 * * *` - Every day at 9:00 AM
- `0 9 * * 1` - Every Monday at 9:00 AM
- `*/15 * * * *` - Every 15 minutes

## Job Management

### Creating a Job

Required fields:
- **name** - Descriptive name for the job
- **schedule** - When to run (natural language or cron)
- **prompt** - What to execute (the task description)
- **channel** - Where to deliver results (optional, default: "cli")

### Example Jobs

**Daily Summary:**
```json
{
  "action": "add",
  "name": "Daily Summary",
  "schedule": "every day at 9am",
  "prompt": "Summarize yesterday's git commits and open PRs",
  "channel": "cli"
}
```

**Health Check:**
```json
{
  "action": "add",
  "name": "Health Check",
  "schedule": "every hour",
  "prompt": "Check disk space and memory usage, alert if low",
  "channel": "cli"
}
```

**Weekly Report:**
```json
{
  "action": "add",
  "name": "Weekly Report",
  "schedule": "0 9 * * 1",
  "prompt": "Generate weekly activity report",
  "channel": "cli"
}
```

### Managing Jobs

- **list** - Show all scheduled jobs with status
- **enable** `<job_id>` - Enable a disabled job
- **disable** `<job_id>` - Pause a job without removing it
- **trigger** `<job_id>` - Run a job immediately (manual execution)
- **remove** `<job_id>` - Delete a job permanently

## Job States

Jobs can be:
- **Enabled** - Will run on schedule
- **Disabled** - Won't run but preserved for later
- **Eligible** - Requirements met (has prompt, valid schedule)
- **Ineligible** - Missing requirements

## Best Practices

1. **Descriptive names** - Make it clear what the job does
2. **Specific prompts** - Be explicit in the prompt text
3. **Test first** - Use "trigger" to test before scheduling
4. **Reasonable intervals** - Don't schedule too frequently
5. **Monitor results** - Check that jobs are running successfully

## Limitations

- Maximum job runtime depends on configuration
- Jobs run in the background without user interaction
- Results are delivered to the specified channel
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skills_created() {
        let skills = get_builtin_skills();
        assert!(skills.contains_key("skill-creator"));
        assert!(skills.contains_key("find-skills"));
        assert!(skills.contains_key("cron"));
    }

    #[test]
    fn test_skill_creator_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("skill-creator").unwrap();

        assert_eq!(skill.name, "skill-creator");
        assert_eq!(skill.metadata.emoji, "🛠️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_find_skills_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("find-skills").unwrap();

        assert_eq!(skill.name, "find-skills");
        assert_eq!(skill.metadata.emoji, "🔍");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
    }

    #[test]
    fn test_cron_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("cron").unwrap();

        assert_eq!(skill.name, "cron");
        assert_eq!(skill.metadata.emoji, "⏰");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
    }
}
