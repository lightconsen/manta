---
name: find-skills
description: "Search for and discover skills across all storage levels"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "find-skills"
    priority: 100
  - type: keyword
    pattern: "find skill"
    priority: 90
  - type: keyword
    pattern: "search skill"
    priority: 90
  - type: keyword
    pattern: "list skills"
    priority: 80
  - type: keyword
    pattern: "show skills"
    priority: 80
openclaw:
  emoji: "🔍"
  category: "system"
  tags:
    - "skills"
    - "search"
    - "discovery"
---

# Find Skills

Search for and discover skills across all storage levels in Manta.

## Storage Levels

Skills can exist at different levels (checked in priority order):

1. **Project** (./.manta/skills/) - Project-specific skills
2. **Workspace** (.manta/skills/) - Workspace-wide skills
3. **User** (~/.manta/skills/) - User-global skills
4. **Bundled** (built-in) - Default system skills

## Search Capabilities

### By Name
Search skill names with partial matching:
- "git" matches: github, gitlab, git-summary

### By Trigger
Find skills by their trigger patterns:
- Command triggers: `/weather`, `/github`
- Keyword triggers: "weather", "forecast"

### By Category
Filter by skill categories:
- dev-tools: Development utilities
- productivity: Task management, notes
- communication: Messaging, email
- media: Images, video, audio
- system: System administration
- data: Databases, analytics
- ai: AI/ML specific tools

### By Tag
Filter by specific tags for fine-grained discovery.

## Usage Examples

### List all skills
```
/find-skills
```

### Search by name
```
find skill github
```

### Filter by category
```
show skills in dev-tools
```

### Show skill details
```
info github
```

## Output Format

```
🔍 Found 3 skills matching "git":

🐙 github (dev-tools)
   GitHub operations via gh CLI
   Triggers: /github, gh, pr, issue

🦊 gitlab (dev-tools)
   GitLab operations
   Triggers: /gitlab, gl

📊 git-summary (productivity)
   Summarize git activity
   Triggers: /git-summary, git log
```

## Best Practices

1. Use specific keywords to find relevant skills
2. Check skill eligibility before use
3. Install missing dependencies for skills
4. Override bundled skills by creating user versions
