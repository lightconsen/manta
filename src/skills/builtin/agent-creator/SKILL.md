---
name: agent-creator
description: "Create, configure, and deploy new AI agents with custom personalities and tools"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "create agent"
    priority: 100
  - type: keyword
    pattern: "new agent"
    priority: 90
  - type: keyword
    pattern: "configure agent"
    priority: 80
  - type: keyword
    pattern: "deploy agent"
    priority: 70
openclaw:
  emoji: "🤖"
  category: "meta"
  tags:
    - "agent"
    - "create"
    - "configure"
    - "deploy"
---

# Agent Creator Skill

Create and configure new AI agents with specialized roles, personalities, and tool sets.

## Capabilities

- Define agent personality, role, and system prompt
- Configure allowed tools and skill sets
- Set iteration limits and safety guardrails
- Deploy agents to specific channels
- Clone and customize existing agents

## Usage Examples

### Create a specialized agent
"Create a code review agent that only uses file reading tools"

### Configure an agent
"Set up a customer support agent with a friendly tone and FAQ access"

### Deploy to channel
"Deploy the support agent to the Telegram channel"

### Clone an agent
"Create a copy of the main agent but restrict it to read-only tools"

## Agent Configuration

```yaml
name: my-agent
personality: |
  You are a helpful assistant specialized in...
model: claude-sonnet
tools:
  - file_read
  - web_search
max_iterations: 10
channels:
  - telegram
  - discord
```

## Deployment Steps

1. Define the agent's role and personality
2. Select appropriate tools and skills
3. Set safety limits (max iterations, token budget)
4. Test with sample inputs
5. Deploy to desired channels

## Best Practices

1. Keep personalities focused and specific
2. Restrict tools to minimum needed for the task
3. Set conservative iteration limits initially
4. Test edge cases before production deployment
5. Monitor new agents closely after launch
