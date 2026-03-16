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

    let agent_browser_skill = create_agent_browser_skill();
    skills.insert(agent_browser_skill.name.clone(), agent_browser_skill);

    let api_gateway_skill = create_api_gateway_skill();
    skills.insert(api_gateway_skill.name.clone(), api_gateway_skill);

    let nano_pdf_skill = create_nano_pdf_skill();
    skills.insert(nano_pdf_skill.name.clone(), nano_pdf_skill);

    let self_improving_agent_skill = create_self_improving_agent_skill();
    skills.insert(self_improving_agent_skill.name.clone(), self_improving_agent_skill);

    let agent_creator_skill = create_agent_creator_skill();
    skills.insert(agent_creator_skill.name.clone(), agent_creator_skill);

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
- **User**: `~/.manta/skills/` - Available everywhere
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

1. Create directory: `mkdir -p ~/.manta/skills/{name}`
2. Create SKILL.md: `touch ~/.manta/skills/{name}/SKILL.md`
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
3. **User** (`~/.manta/skills/`) - Your personal skills
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
npx --yes clawhub@latest install <skill-slug> --workdir ~/.manta
```

Important: Always include `--workdir ~/.manta` so skills install to the correct location.

### Update Installed Skills

```bash
npx --yes clawhub@latest update --all --workdir ~/.manta
```

### List Installed Skills

```bash
npx --yes clawhub@latest list --workdir ~/.manta
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

/// Create the agent-browser built-in skill
fn create_agent_browser_skill() -> Skill {
    let mut skill = Skill::new(
        "agent-browser",
        "Navigate and interact with websites on behalf of the user",
        AGENT_BROWSER_PROMPT,
    )
    .with_emoji("🌐")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "browse".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "browse to".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "go to website".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "visit".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "navigate to".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "web".to_string(),
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

/// Agent Browser skill prompt
const AGENT_BROWSER_PROMPT: &str = r#"# Agent Browser - Web Navigation

Navigate and interact with websites programmatically on behalf of the user.

## When to Use

Use this skill when the user asks:
- "browse to [website]"
- "go to [URL]"
- "visit [website]"
- "navigate to [page]"
- "open [website] in browser"
- "check what's on [website]"

## Capabilities

### Fetch Web Pages

Use the `web_fetch` tool to retrieve page content:

```json
{
  "url": "https://example.com"
}
```

The tool returns the page content as markdown, making it easy to read and analyze.

### Search and Navigate

1. **Direct URL Access**: If the user provides a full URL, fetch it directly
2. **Search First**: If the user provides a partial name, use `web_search` to find the correct URL
3. **Follow Links**: Extract links from pages to navigate deeper

## Workflow

### Basic Browsing

1. User says: "Browse to github.com"
2. Use `web_fetch` with `https://github.com`
3. Summarize the page content for the user

### Searching for Sites

1. User says: "Go to the Rust programming language website"
2. Use `web_search` with query "Rust programming language official"
3. Identify the correct URL from search results
4. Use `web_fetch` to retrieve the site
5. Present the content

### Multi-Step Navigation

For complex tasks:
1. Fetch the initial page
2. Identify the relevant link
3. Fetch the linked page
4. Continue as needed

## Best Practices

1. **Verify URLs**: Always check if the URL is valid before fetching
2. **Handle errors**: If a page fails to load, try searching for alternatives
3. **Respect limits**: Some sites may block automated access
4. **Summarize**: Don't dump raw HTML; extract relevant information
5. **Security**: Never submit forms with sensitive data unless explicitly requested

## Output Format

Present findings clearly:

```
🌐 [Page Title]
URL: [URL]

**Summary**: Brief overview of the page

**Key Content**:
- Point 1
- Point 2

**Links of Interest**:
- [Link description](URL)
```

## Examples

**Example 1**: "Browse to example.com"
```
🌐 Example Domain
URL: https://example.com

This domain is for use in illustrative examples in documents.
```

**Example 2**: "What's on the front page of Hacker News?"
1. Fetch https://news.ycombinator.com
2. Extract top stories
3. Present as a numbered list with links
"#;

/// Create the api-gateway built-in skill
fn create_api_gateway_skill() -> Skill {
    let mut skill = Skill::new(
        "api-gateway",
        "Design, test, and manage API endpoints and integrations",
        API_GATEWAY_PROMPT,
    )
    .with_emoji("🚪")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "api".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "api endpoint".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "rest api".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "api design".to_string(),
            priority: 85,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "test api".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "curl".to_string(),
            priority: 70,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "http request".to_string(),
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

/// API Gateway skill prompt
const API_GATEWAY_PROMPT: &str = r#"# API Gateway - API Design & Testing

Design, test, and manage API endpoints and integrations using HTTP requests.

## When to Use

Use this skill when the user asks:
- "test this API endpoint"
- "design an API for..."
- "make a POST request to..."
- "curl this URL"
- "api endpoint for..."
- "rest api design"

## Tools Available

### HTTP Requests with curl

Use `shell` tool to execute curl commands:

```bash
curl -X [METHOD] [URL] [OPTIONS]
```

### Common Options:

```bash
# GET request (default)
curl https://api.example.com/users

# POST with JSON data
curl -X POST https://api.example.com/users \
  -H "Content-Type: application/json" \
  -d '{"name": "John", "email": "john@example.com"}'

# With authentication
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/protected

# With query parameters
curl "https://api.example.com/search?q=hello&limit=10"

# Save response to file
curl -o response.json https://api.example.com/data

# Show response headers
curl -I https://api.example.com/users

# Follow redirects
curl -L https://bit.ly/xxx
```

## API Design Guidelines

### RESTful Principles

1. **Resources**: Use nouns, not verbs
   - ✅ `/users`, `/orders`, `/products`
   - ❌ `/getUsers`, `/createOrder`

2. **HTTP Methods**:
   - `GET` - Read
   - `POST` - Create
   - `PUT` - Update (full)
   - `PATCH` - Update (partial)
   - `DELETE` - Remove

3. **Status Codes**:
   - `200` - OK
   - `201` - Created
   - `400` - Bad Request
   - `401` - Unauthorized
   - `404` - Not Found
   - `500` - Server Error

4. **Versioning**:
   - URL: `/v1/users`
   - Header: `Accept: application/vnd.api.v1+json`

### Example API Design

```yaml
# User API

## Endpoints

GET    /api/v1/users          # List users
POST   /api/v1/users          # Create user
GET    /api/v1/users/:id      # Get user
PUT    /api/v1/users/:id      # Update user
DELETE /api/v1/users/:id      # Delete user

## Request/Response Examples

POST /api/v1/users
Request:
{
  "name": "Jane Doe",
  "email": "jane@example.com"
}

Response (201 Created):
{
  "id": "123",
  "name": "Jane Doe",
  "email": "jane@example.com",
  "created_at": "2024-01-15T10:30:00Z"
}
```

## Testing APIs

### Step-by-Step Testing

1. **Start with GET**: Test read endpoints first
2. **Check headers**: Verify Content-Type, auth tokens
3. **Test error cases**: Invalid inputs, missing auth
4. **Validate responses**: Check structure and data types

### Example Test Session

```bash
# 1. Test GET
curl https://jsonplaceholder.typicode.com/posts/1

# 2. Test POST
curl -X POST https://jsonplaceholder.typicode.com/posts \
  -H "Content-Type: application/json" \
  -d '{"title": "Test", "body": "Content", "userId": 1}'

# 3. Test with auth (if needed)
curl -H "Authorization: Bearer $TOKEN" \
  https://api.example.com/protected
```

## Best Practices

1. **Use environment variables** for secrets
2. **Pretty-print JSON** with `| jq .` if available
3. **Document response formats** clearly
4. **Handle rate limits** - watch for 429 status
5. **Test edge cases** - empty inputs, special characters

## Security

- Never hardcode API keys in commands
- Use HTTPS for production APIs
- Be careful with PUT/DELETE - they modify data
- Sanitize user input before using in URLs
"#;

/// Create the nano-pdf built-in skill
fn create_nano_pdf_skill() -> Skill {
    let mut skill = Skill::new(
        "nano-pdf",
        "Read, create, and manipulate PDF documents",
        NANO_PDF_PROMPT,
    )
    .with_emoji("📄")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "pdf".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "pdf".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "read pdf".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "create pdf".to_string(),
            priority: 85,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "extract text".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "pdf to text".to_string(),
            priority: 80,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    // Note: requires_bin takes ownership, must chain before assigning to skill
    let mut skill = skill
        .requires_bin("pdftotext")
        .requires_bin("pandoc");

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Nano PDF skill prompt
const NANO_PDF_PROMPT: &str = r#"# Nano PDF - PDF Document Processing

Read, create, and manipulate PDF documents using command-line tools.

## When to Use

Use this skill when the user asks:
- "read this PDF"
- "extract text from pdf"
- "create a PDF from..."
- "convert pdf to text"
- "pdf to markdown"
- "search in pdf"

## Requirements

Optional tools (will limit functionality if not available):
- `pdftotext` - Part of poppler-utils (text extraction)
- `pandoc` - Document conversion
- `pdfinfo` - PDF metadata

## Capabilities

### Extract Text from PDF

```bash
# Basic text extraction
pdftotext input.pdf output.txt

# Keep layout
pdftotext -layout input.pdf output.txt

# Extract specific page range
pdftotext -f 1 -l 5 input.pdf output.txt  # Pages 1-5

# Extract to stdout
pdftotext - input.pdf -

# Raw mode (no formatting)
pdftotext -raw input.pdf output.txt
```

### PDF Information

```bash
# Get metadata
pdfinfo input.pdf

# List form fields
pdfinfo -meta input.pdf
```

### Convert PDF to Other Formats

```bash
# PDF to HTML
pdftohtml input.pdf output.html

# PDF to images (one per page)
pdftoppm input.pdf output -png

# Using pandoc for complex conversions
pandoc input.pdf -t markdown -o output.md
```

### Create PDF from Other Formats

```bash
# Markdown to PDF (via pandoc)
pandoc input.md -o output.pdf

# HTML to PDF
pandoc input.html -o output.pdf

# Text to PDF
enscript -p - input.txt | ps2pdf - output.pdf
```

### Merge and Split PDFs

```bash
# Merge PDFs (requires pdftk or qpdf)
pdftk file1.pdf file2.pdf cat output merged.pdf

# Or using qpdf
qpdf --empty --pages file1.pdf file2.pdf -- merged.pdf

# Split PDF (extract specific pages)
pdftk input.pdf cat 1-5 output first_five.pdf
```

## Reading PDF Content

When a user asks to read a PDF:

1. **Check if file exists** using file tools
2. **Extract text** using `pdftotext`
3. **Summarize** the content for the user
4. **Answer questions** about the content

### Example Workflow

User: "Read this PDF document.pdf"

Steps:
1. Verify file exists
2. Extract text: `pdftotext document.pdf -`
3. Present summary with key points
4. Offer to search for specific terms

## Creating PDFs

### From Markdown

```bash
# Simple approach
pandoc input.md -o output.pdf

# With styling
pandoc input.md -o output.pdf --pdf-engine=xelatex -V geometry:margin=1in
```

### From Code/Text Output

```bash
# Pretty-print code to PDF
enscript -p - -Ejavascript code.js | ps2pdf - code.pdf
```

## Best Practices

1. **Check tool availability** before using (pdftotext, pandoc)
2. **Handle large PDFs** - consider page ranges for big documents
3. **Preserve formatting** when needed (-layout flag)
4. **Respect scanned PDFs** - OCR may be needed for image-based PDFs
5. **Clean up** temporary files after processing

## Limitations

- `pdftotext` works best on text-based PDFs
- Image/scanned PDFs require OCR (not supported natively)
- Complex layouts may lose formatting
- Password-protected PDFs cannot be processed

## Tips

- Use `-layout` to preserve columns and tables
- Pipe to `head` or `tail` for preview: `pdftotext -layout doc.pdf - | head -50`
- Check page count first with `pdfinfo` for large documents
- For code/docs, consider Markdown → PDF via pandoc
"#;

/// Create the self-improving-agent built-in skill
fn create_self_improving_agent_skill() -> Skill {
    let mut skill = Skill::new(
        "self-improving-agent",
        "Analyze and improve Manta's own performance and configuration",
        SELF_IMPROVING_AGENT_PROMPT,
    )
    .with_emoji("🔄")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "self-improve".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "improve yourself".to_string(),
            priority: 95,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "self improve".to_string(),
            priority: 95,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "analyze performance".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "optimize manta".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "fix your config".to_string(),
            priority: 85,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "debug yourself".to_string(),
            priority: 85,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "system check".to_string(),
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

/// Self-Improving Agent skill prompt
const SELF_IMPROVING_AGENT_PROMPT: &str = r#"# Self-Improving Agent - System Analysis & Optimization

Analyze Manta's performance, configuration, and behavior to suggest and implement improvements.

## When to Use

Use this skill when the user asks:
- "improve yourself"
- "analyze your performance"
- "optimize manta"
- "fix your config"
- "debug yourself"
- "system check"
- "self improve"

## Capabilities

### 1. Configuration Analysis

Check Manta's configuration files for issues:
- `~/.manta/config.yaml` - Main config
- `~/.manta/skills/` - User skills
- `./.manta/skills/` - Project skills

### 2. Log Analysis

Analyze recent logs to identify issues:
```bash
# View recent logs (if available)
tail -n 100 ~/.local/share/manta/logs/manta.log
```

### 3. Performance Metrics

Check system resources:
```bash
# Memory usage
ps aux | grep manta

# Disk space
df -h ~/.manta

# Large files in manta directories
find ~/.manta -type f -size +10M
```

### 4. Skill Health Check

Verify installed skills:
- Check for broken skill files
- Identify duplicate skills
- Find skills with missing requirements
- List unused skills

## Analysis Workflow

### Step 1: Gather Information

When user triggers self-improvement:
1. Check current configuration
2. List installed skills
3. Check for errors in recent sessions
4. Analyze resource usage

### Step 2: Identify Issues

Look for common problems:
- **Config Issues**: Missing API keys, invalid settings
- **Skill Issues**: Ineligible skills, missing binaries
- **Performance**: Slow response times, high memory usage
- **Storage**: Old logs, temporary files, duplicates

### Step 3: Generate Recommendations

Create actionable suggestions:
```
🔧 Self-Improvement Report

📊 Current State:
- Skills loaded: X
- Eligible skills: Y
- Config file: OK/Missing/Issues

⚠️ Issues Found:
1. [Issue description]
   → [Suggested fix]

2. [Issue description]
   → [Suggested fix]

💡 Recommendations:
1. [Recommendation]
2. [Recommendation]
```

### Step 4: Apply Fixes (with permission)

Ask user before making changes:
- "Should I clean up old log files?"
- "Can I disable the broken X skill?"
- "Shall I update your config with Y?"

## Common Improvements

### Cleanup Tasks

```bash
# Clean old logs (>30 days)
find ~/.local/share/manta/logs -name "*.log" -mtime +30 -delete

# Remove empty skill directories
find ~/.manta/skills -type d -empty -delete

# Clear temporary files
rm -rf /tmp/manta-*
```

### Config Optimization

Suggest optimal settings based on usage:
- Model selection for hardware
- Timeout adjustments
- Token limits
- Provider preferences

### Skill Management

- Disable unused skills
- Update outdated skills
- Fix skill requirements
- Consolidate duplicate functionality

## Safety Guidelines

1. **Always ask permission** before modifying files
2. **Backup before changes** - suggest creating backups
3. **Explain the rationale** - why is this change needed?
4. **Test after changes** - verify fixes work
5. **Respect user preferences** - honor explicit settings

## Output Format

Present findings clearly:

```
🔄 Self-Improvement Analysis

📊 System Health: [Good/Warning/Critical]

🔍 Analysis Results:
[Detailed findings]

⚡ Quick Wins:
1. [Easy improvement]
2. [Easy improvement]

🔧 Recommended Actions:
1. [Action with explanation]
   Run: [command if applicable]

2. [Action with explanation]

📝 Config Suggestions:
```yaml
# Add to ~/.manta/config.yaml
setting: value
```

Would you like me to apply any of these fixes?
```

## Self-Monitoring Commands

Useful commands for self-analysis:

```bash
# Check if manta is running
pgrep -f manta

# Memory usage
ps -o pid,rss,vsz,comm -p $(pgrep -f manta)

# Open files
lsof -p $(pgrep -f manta) 2>/dev/null | head -20

# Config syntax check
cat ~/.manta/config.yaml | head -20

# Disk usage of manta directories
du -sh ~/.manta ~/.local/share/manta ~/.cache/manta 2>/dev/null
```

## Limitations

- Cannot restart the daemon process itself
- Cannot modify compiled code
- Cannot access external APIs without keys
- Changes require user confirmation

## Example Session

**User**: "improve yourself"

**Manta**:
```
🔄 Self-Improvement Analysis

📊 System Health: Good

📋 Summary:
- 12 built-in skills active
- 3 user skills loaded
- Config file: OK
- Memory usage: Normal

💡 Recommendations:
1. Found 3 skills with missing binaries (nano-pdf)
   → Install pdftotext and pandoc for full PDF support

2. Log files taking 50MB
   → Run cleanup to remove logs older than 30 days?

3. No API key configured for web search
   → Add SEARCH_API_KEY to config for better results

Would you like me to help with any of these?
```
"#;

/// Create the agent-creator built-in skill
fn create_agent_creator_skill() -> Skill {
    let mut skill = Skill::new(
        "agent-creator",
        "Create new agent personalities using natural language",
        AGENT_CREATOR_PROMPT,
    )
    .with_emoji("🎭")
    .by("manta");

    skill.triggers = vec![
        SkillTrigger {
            trigger_type: TriggerType::Command,
            pattern: "create-agent".to_string(),
            priority: 100,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "create agent".to_string(),
            priority: 95,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "new agent".to_string(),
            priority: 95,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "make an agent".to_string(),
            priority: 90,
            user_invocable: true,
            model_invocable: true,
        },
        SkillTrigger {
            trigger_type: TriggerType::Keyword,
            pattern: "agent personality".to_string(),
            priority: 85,
            user_invocable: true,
            model_invocable: true,
        },
    ];

    skill.source_level = StorageLevel::Bundled;
    skill.is_eligible = true;
    skill.enabled = true;

    skill
}

/// Agent Creator skill prompt
const AGENT_CREATOR_PROMPT: &str = r#"# Agent Creator - Create New Agent Personalities

Create new agent personalities using natural language. This skill parses your description and creates OpenClaw-style memory files (SOUL.md, IDENTITY.md, BOOTSTRAP.md).

## When to Use

Use this skill when the user says:
- "create agent named X"
- "make an agent that does Y"
- "new agent personality for Z"
- "agent that behaves like..."
- "/create-agent"

## How It Works

You extract information from the user's request and use the `file_write` tool to create the three memory files.

## File Locations

Agents are stored in: `~/.manta/agents/<agent-name>/`

## Required Files

### 1. IDENTITY.md
Contains the agent's identity information:
```markdown
# Agent Identity

## Name
<Display Name>

## Role
<Role description>

## Communication Style
<style: concise/detailed/friendly/professional/technical>

## Created
<timestamp>
```

### 2. SOUL.md
Contains personality, values, and behavior:
```markdown
# Agent Soul

## Core Values
- <Value 1>
- <Value 2>

## Behavioral Guidelines
- <Behavior 1>
- <Behavior 2>

## Expertise
<Domain expertise>

## Communication Style
- Tone: <tone>
- Vocabulary: <vocabulary level>
- Response Length: <length preference>
```

### 3. BOOTSTRAP.md
Contains startup behavior and system prompt:
```markdown
# Bootstrap Configuration

## System Prompt
<Full system prompt for the agent>

## Initial Greeting
<Optional greeting message>

## Startup Behavior
- <Behavior 1>
- <Behavior 2>
```

## Workflow

### Step 1: Extract Information

From the user's request, extract:
- **Name**: Agent identifier (directory name) - use lowercase, no spaces
- **Display Name**: Human-readable name
- **Role**: What the agent does (e.g., "Code Reviewer", "Creative Writer")
- **Style**: Communication style (concise, detailed, friendly, professional, technical)
- **Expertise**: Domain knowledge areas
- **Purpose**: What tasks this agent will handle

### Step 2: Create Directory

Use `shell` tool to create the agent directory:
```bash
mkdir -p ~/.manta/agents/<agent-name>
```

### Step 3: Write Memory Files

Use `file_write` tool to create all three files.

### Step 4: Confirm Success

Report what was created and how to use it.

## Examples

### Example 1: Code Reviewer Agent

**User**: "Create an agent named 'codereview' that acts as a strict senior code reviewer"

**Your Actions**:
1. Create directory: `mkdir -p ~/.manta/agents/codereview`
2. Write IDENTITY.md with name "Code Reviewer", role "Senior Code Reviewer"
3. Write SOUL.md with values: thoroughness, security, performance
4. Write BOOTSTRAP.md with strict code review prompt

### Example 2: Creative Writer Agent

**User**: "Make an agent for creative writing with a friendly, encouraging style"

**Your Actions**:
1. Create directory: `mkdir -p ~/.manta/agents/creative`
2. Write IDENTITY.md with name "Creative Muse", style "friendly"
3. Write SOUL.md emphasizing creativity, encouragement, brainstorming
4. Write BOOTSTRAP.md with creative writing focus

### Example 3: Minimal Request

**User**: "new agent named helper"

**Your Actions**:
1. Create with defaults: role "AI Assistant", style "professional"
2. Write all three files with sensible defaults

## Best Practices

1. **Derive from description**: Infer personality traits from user's description
2. **Be specific**: Tailor system prompt to the specific role
3. **Consistent naming**: Use kebab-case for agent names (e.g., "code-reviewer")
4. **Validate**: Confirm files were created successfully
5. **Show usage**: Tell user how to activate the agent

## Output Format

After creating the agent, report:

```
🎭 Created Agent: <name>

📋 Details:
   Name: <display name>
   Role: <role>
   Style: <style>

📁 Files Created:
   ~/.manta/agents/<name>/
   ├── SOUL.md
   ├── IDENTITY.md
   └── BOOTSTRAP.md

🚀 To Use:
   manta agent set <name>

   Or via web terminal:
   "switch to <name> agent"
```

## Error Handling

- If agent already exists: Ask if user wants to overwrite
- If directory creation fails: Report error with path
- If file write fails: Clean up and report

## Important Notes

- Agent names should be lowercase with hyphens (not spaces)
- Default location is ~/.manta/agents/
- Agent won't be active until user runs `manta agent set <name>`
- Files follow OpenClaw specification
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
        assert!(skills.contains_key("agent-browser"));
        assert!(skills.contains_key("api-gateway"));
        assert!(skills.contains_key("nano-pdf"));
        assert!(skills.contains_key("self-improving-agent"));
        assert!(skills.contains_key("agent-creator"));
        assert_eq!(skills.len(), 13);
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

    #[test]
    fn test_agent_browser_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("agent-browser").unwrap();

        assert_eq!(skill.name, "agent-browser");
        assert_eq!(skill.metadata.emoji, "🌐");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_api_gateway_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("api-gateway").unwrap();

        assert_eq!(skill.name, "api-gateway");
        assert_eq!(skill.metadata.emoji, "🚪");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_nano_pdf_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("nano-pdf").unwrap();

        assert_eq!(skill.name, "nano-pdf");
        assert_eq!(skill.metadata.emoji, "📄");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
        // Nano PDF requires pdftotext and pandoc
        assert!(skill.metadata.requires.bins.contains(&"pdftotext".to_string()));
    }

    #[test]
    fn test_self_improving_agent_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("self-improving-agent").unwrap();

        assert_eq!(skill.name, "self-improving-agent");
        assert_eq!(skill.metadata.emoji, "🔄");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_agent_creator_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("agent-creator").unwrap();

        assert_eq!(skill.name, "agent-creator");
        assert_eq!(skill.metadata.emoji, "🎭");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }
}
