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

    let clawhub_skill = create_clawhub_skill();
    skills.insert(clawhub_skill.name.clone(), clawhub_skill);

    let summarize_skill = create_summarize_skill();
    skills.insert(summarize_skill.name.clone(), summarize_skill);

    let weather_skill = create_weather_skill();
    skills.insert(weather_skill.name.clone(), weather_skill);

    let tmux_skill = create_tmux_skill();
    skills.insert(tmux_skill.name.clone(), tmux_skill);

    let github_skill = create_github_skill();
    skills.insert(github_skill.name.clone(), github_skill);

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

/// Create the clawhub built-in skill
fn create_clawhub_skill() -> Skill {
    let mut skill = Skill::new(
        "clawhub",
        "Search and install skills from ClawHub public registry",
        CLAWHUB_PROMPT,
    )
    .with_emoji("🦞")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "clawhub".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "install skill".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "search clawhub".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "clawhub search".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the summarize built-in skill
fn create_summarize_skill() -> Skill {
    let mut skill = Skill::new(
        "summarize",
        "Summarize URLs, files, and content",
        SUMMARIZE_PROMPT,
    )
    .with_emoji("📝")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "summarize".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "summarize".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "tl;dr".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "give me a summary".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the weather built-in skill
fn create_weather_skill() -> Skill {
    let mut skill = Skill::new(
        "weather",
        "Get weather information for locations",
        WEATHER_PROMPT,
    )
    .with_emoji("🌤️")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "weather".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "weather".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "forecast".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "temperature".to_string(),
            priority: 70,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the tmux built-in skill
fn create_tmux_skill() -> Skill {
    let mut skill = Skill::new(
        "tmux",
        "Control tmux sessions remotely",
        TMUX_PROMPT,
    )
    .with_emoji("🖥️")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "tmux".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "tmux".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "terminal session".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Create the github built-in skill
fn create_github_skill() -> Skill {
    let mut skill = Skill::new(
        "github",
        "GitHub CLI integration for repos, PRs, and issues",
        GITHUB_PROMPT,
    )
    .with_emoji("🐙")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "github".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "github".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "pull request".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "create pr".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "gh ".to_string(),
            priority: 70,
            user_invocable: true,
            model_invocable: true,
        },
    ];

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

/// ClawHub skill prompt
const CLAWHUB_PROMPT: &str = r#"# ClawHub - Public Skill Registry

Search and install skills from ClawHub (clawhub.ai), the public skill registry for OpenClaw-based agents.

## When to Use

Use this skill when the user asks:
- "find a skill for..."
- "search for skills"
- "install a skill"
- "what skills are available?"
- "update my skills"

## Commands

### Search for Skills

```bash
npx --yes clawhub@latest search "<query>" --limit 5
```

Examples:
- `npx --yes clawhub@latest search "web scraping"`
- `npx --yes clawhub@latest search "postgres" --limit 10`

### Install a Skill

```bash
npx --yes clawhub@latest install <skill-slug> --workdir ~/.config/manta
```

Important: Always include `--workdir ~/.config/manta` so skills install to the correct location.

### Update Installed Skills

```bash
npx --yes clawhub@latest update --all --workdir ~/.config/manta
```

### List Installed Skills

```bash
npx --yes clawhub@latest list --workdir ~/.config/manta
```

## Requirements

- Node.js must be installed (for npx)
- No API key needed for search/install
- Login only required for publishing skills

## After Installation

1. Confirm the skill was installed successfully
2. Remind the user to start a new session to load the skill
3. Suggest using `/find-skills` to verify the skill appears

## Notes

- ClawHub is a public registry - all skills are open and visible
- Skills are versioned with semver
- The registry uses vector search (not just keywords)
"#;

/// Summarize skill prompt
const SUMMARIZE_PROMPT: &str = r#"# Summarize - Content Summarization

Summarize URLs, files, and text content concisely.

## When to Use

Use this skill when the user asks:
- "summarize this URL"
- "give me a summary of..."
- "tl;dr"
- "what's the main point of..."

## Capabilities

### Summarize a URL

Use `web_fetch` tool to fetch the content, then provide a summary:

1. Fetch the URL content
2. Extract the main points
3. Present a concise summary (3-5 bullet points for long content)

### Summarize a File

1. Read the file using file tools
2. Identify key sections and main points
3. Provide a structured summary

### Summarize Text

If the user provides text directly:
1. Identify the topic and key points
2. Condense while preserving meaning
3. Format for readability

## Summary Format

For most summaries, use this structure:

```
**Summary**: One-sentence overview

**Key Points**:
- Point 1
- Point 2
- Point 3

**Details** (if relevant):
Brief elaboration on important aspects
```

## Tips

- Adjust detail level based on content length and user needs
- Preserve technical accuracy when summarizing code/docs
- Highlight actionable items if present
- Note the date/source if relevant for timeliness
"#;

/// Weather skill prompt
const WEATHER_PROMPT: &str = r#"# Weather - Weather Information

Get weather information for any location using wttr.in or Open-Meteo APIs.

## When to Use

Use this skill when the user asks:
- "what's the weather like?"
- "weather in [city]"
- "forecast for..."
- "temperature in..."
- "will it rain today?"

## Commands

### Current Weather (wttr.in)

```bash
curl -s "wttr.in/<location>?format=3"
```

Examples:
- `curl -s "wttr.in/New York?format=3"`
- `curl -s "wttr.in/London?format=3"`
- `curl -s "wttr.in/90210?format=3"` (zip code)

### Detailed Weather

```bash
curl -s "wttr.in/<location>" | head -17
```

This shows the full weather report with forecast.

### JSON Format (for parsing)

```bash
curl -s "wttr.in/<location>?format=j1" | jq '.current_condition[0]'
```

## Output Format

Present weather clearly:

```
🌤️ Weather in [Location]

Current: [temp]°C / [temp]°F, [conditions]
Feels like: [temp]°C
Humidity: [x]%
Wind: [speed] [direction]

Forecast:
- Today: [conditions], high/low
- Tomorrow: [conditions], high/low
```

## Notes

- wttr.in is free and requires no API key
- Location can be city name, airport code (e.g., JFK), or zip code
- For ambiguous locations, ask the user to clarify
- Include both Celsius and Fahrenheit for clarity
"#;

/// Tmux skill prompt
const TMUX_PROMPT: &str = r#"# Tmux - Terminal Session Management

Control tmux sessions remotely for long-running tasks and persistent terminals.

## When to Use

Use this skill when the user asks:
- "start a tmux session"
- "attach to tmux"
- "list tmux sessions"
- "run this in the background"
- "keep this running after I disconnect"

## Common Commands

### Session Management

```bash
# List all sessions
tmux ls

# Create new session
tmux new-session -d -s <session-name>

# Create session and run command
tmux new-session -d -s <session-name> "<command>"

# Attach to session
tmux attach -t <session-name>

# Detach from session (inside tmux)
Ctrl+b d

# Kill session
tmux kill-session -t <session-name>

# Kill all sessions
tmux kill-server
```

### Window Management

```bash
# New window (inside tmux)
Ctrl+b c

# Next window
Ctrl+b n

# Previous window
Ctrl+b p

# List windows
Ctrl+b w

# Rename window
Ctrl+b ,
```

### Pane Management

```bash
# Split horizontally
Ctrl+b %

# Split vertically
Ctrl+b "

# Navigate panes
Ctrl+b <arrow-key>

# Close pane
Ctrl+b x
```

### Send Commands to Session

```bash
# Send keys to session without attaching
tmux send-keys -t <session-name> "<command>" Enter

# Example: Run a long build in background
tmux send-keys -t build "cargo build --release" Enter
```

## Use Cases

### Long-Running Builds

```bash
tmux new-session -d -s build "cargo build --release"
# Later, check status:
tmux capture-pane -t build -p | tail -20
```

### Persistent Development Server

```bash
tmux new-session -d -s devserver "npm run dev"
```

### Monitoring/Logs

```bash
tmux new-session -d -s logs "tail -f /var/log/app.log"
```

## Best Practices

1. Name sessions descriptively (e.g., "build", "server", "logs")
2. Use `tmux ls` to show running sessions
3. Check session output with `tmux capture-pane -t <name> -p`
4. Remind users they can reattach with `tmux attach -t <name>`

## Notes

- tmux must be installed on the system
- Sessions survive disconnections
- Perfect for remote work over SSH
"#;

/// GitHub skill prompt
const GITHUB_PROMPT: &str = r#"# GitHub - GitHub CLI Integration

Work with GitHub repositories, PRs, and issues using the `gh` CLI.

## When to Use

Use this skill when the user asks:
- "create a pull request"
- "check my github issues"
- "gh ..."
- "github status"
- "view PRs"

## Prerequisites

The `gh` CLI must be installed and authenticated:
- Install: https://cli.github.com/
- Auth: `gh auth login`

## Common Commands

### Repository Operations

```bash
# Clone a repo
gh repo clone <owner>/<repo>

# Create a new repo
gh repo create <name> --public
gh repo create <name> --private

# View repo in browser
gh repo view --web

# List repos for user/org
gh repo list <username>
```

### Pull Requests

```bash
# List PRs
gh pr list
gh pr list --state open
gh pr list --state merged

# View PR details
gh pr view <number>
gh pr view <number> --web

# Create a PR
gh pr create --title "Title" --body "Description"
gh pr create --fill  # Use commit message

# Checkout a PR locally
gh pr checkout <number>

# Review PR
gh pr review <number> --approve
gh pr review <number> --request-changes --body "Feedback"

# Merge PR
gh pr merge <number>
gh pr merge <number> --squash
gh pr merge <number> --rebase
```

### Issues

```bash
# List issues
gh issue list
gh issue list --state open
gh issue list --label bug

# View issue
gh issue view <number>

# Create issue
gh issue create --title "Title" --body "Description"
gh issue create --label bug --label urgent

# Close issue
gh issue close <number>
gh issue close <number> --comment "Fixed in PR #123"
```

### Workflow/Actions

```bash
# List workflows
gh workflow list

# Run workflow
gh workflow run <name>

# View workflow runs
gh run list
gh run view <run-id>
gh run logs <run-id>
```

### Code Review

```bash
# View diff
gh pr diff <number>

# Comment on PR
gh pr comment <number> --body "LGTM!"
```

## Best Practices

1. Check if `gh` is installed before using: `which gh`
2. Ensure user is authenticated: `gh auth status`
3. Use `--web` flag to open in browser when appropriate
4. For PR creation, suggest `--fill` to use commit message as body

## Integration with Git

Common workflow:
```bash
# Start work on new feature
git checkout -b feature-branch
# ... make changes ...
git add .
git commit -m "Add feature"
git push -u origin feature-branch

# Create PR
gh pr create --fill
```

## Notes

- `gh` provides a cleaner interface than raw git for GitHub-specific features
- Always confirm destructive actions (closing PRs/issues)
- Use `gh api` for advanced GraphQL/REST API calls if needed
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
        assert!(skills.contains_key("clawhub"));
        assert!(skills.contains_key("summarize"));
        assert!(skills.contains_key("weather"));
        assert!(skills.contains_key("tmux"));
        assert!(skills.contains_key("github"));
        assert_eq!(skills.len(), 8);
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

    #[test]
    fn test_clawhub_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("clawhub").unwrap();

        assert_eq!(skill.name, "clawhub");
        assert_eq!(skill.metadata.emoji, "🦞");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_summarize_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("summarize").unwrap();

        assert_eq!(skill.name, "summarize");
        assert_eq!(skill.metadata.emoji, "📝");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_weather_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("weather").unwrap();

        assert_eq!(skill.name, "weather");
        assert_eq!(skill.metadata.emoji, "🌤️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_tmux_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("tmux").unwrap();

        assert_eq!(skill.name, "tmux");
        assert_eq!(skill.metadata.emoji, "🖥️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_github_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("github").unwrap();

        assert_eq!(skill.name, "github");
        assert_eq!(skill.metadata.emoji, "🐙");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }
}
