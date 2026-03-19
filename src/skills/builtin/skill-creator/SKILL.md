---
name: skill-creator
description: "Create and package new skills for Manta"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "skill-create"
    priority: 100
  - type: keyword
    pattern: "create skill"
    priority: 90
  - type: keyword
    pattern: "new skill"
    priority: 90
  - type: keyword
    pattern: "make skill"
    priority: 90
openclaw:
  emoji: "🛠️"
  category: "system"
  tags:
    - "skills"
    - "creation"
    - "development"
---

# Skill Creator

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
