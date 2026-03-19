---
name: github
description: "GitHub operations via gh CLI"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "github"
    priority: 100
  - type: keyword
    pattern: "github"
    priority: 90
  - type: keyword
    pattern: "gh"
    priority: 80
  - type: keyword
    pattern: "pr"
    priority: 70
  - type: keyword
    pattern: "pull request"
    priority: 70
  - type: keyword
    pattern: "issue"
    priority: 70
openclaw:
  emoji: "🐙"
  category: "dev-tools"
  tags:
    - "git"
    - "github"
    - "version-control"
    - "ci-cd"
  requires:
    bins: ["gh"]
---

# GitHub Skill

Perform GitHub operations using the `gh` CLI tool.

## Prerequisites

The `gh` (GitHub CLI) tool must be installed and authenticated:
```bash
gh auth login
```

## Capabilities

### Repository Operations
- View repository information
- List repositories
- Clone repositories
- Fork repositories

### Pull Request Management
- List open PRs
- View PR details
- Create PRs
- Review PRs
- Merge PRs

### Issue Management
- List issues
- View issue details
- Create issues
- Close issues
- Add labels and comments

### Workflow & Actions
- View workflow runs
- Trigger workflows
- Check action status

## Usage Examples

### Check repository status
```bash
gh repo view --web
```

### List open PRs
```bash
gh pr list --state open
```

### Create a PR
```bash
gh pr create --title "Feature description" --body "Details"
```

### View recent issues
```bash
gh issue list --limit 10 --state open
```

## Best Practices

1. Always check if in a git repository first
2. Use web view for complex operations (`--web` flag)
3. Verify `gh` authentication status
4. Quote multi-word titles and descriptions
