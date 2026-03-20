---
name: self-improving-agent
description: "Analyze performance, learn from interactions, and improve agent behavior over time"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "improve"
    priority: 100
  - type: keyword
    pattern: "self improve"
    priority: 90
  - type: keyword
    pattern: "learn from"
    priority: 70
  - type: keyword
    pattern: "optimize behavior"
    priority: 80
openclaw:
  emoji: "🔄"
  category: "meta"
  tags:
    - "self-improvement"
    - "learning"
    - "optimization"
    - "meta"
---

# Self-Improving Agent Skill

Enable the agent to analyze its own performance and improve over time.

## Capabilities

- Analyze past conversations for patterns and failures
- Identify frequently asked questions and optimize responses
- Extract reusable patterns and save as instincts or skills
- Monitor tool usage and suggest better approaches
- Generate improvement proposals for operator review

## Usage Examples

### Analyze recent sessions
"Review the last 10 conversations and find improvement opportunities"

### Extract patterns
"What patterns have you noticed in how users ask for help?"

### Generate new skill
"Create a skill based on the deployment workflow we keep repeating"

### Review performance
"How well am I handling technical questions? Where do I struggle?"

## Self-Improvement Process

1. **Observation** — Track successful and failed interactions
2. **Analysis** — Identify patterns, gaps, and opportunities
3. **Proposal** — Draft improvements (new skills, updated prompts, better tool use)
4. **Review** — Present proposals for operator approval
5. **Integration** — Apply approved improvements to future sessions

## Output

- Improvement reports with specific recommendations
- Draft SKILL.md files for new patterns
- Updated personality/instruction suggestions
- Tool usage optimization recommendations

## Best Practices

1. Always require human review before applying changes
2. Track metrics before and after improvements
3. Version-control all changes for rollback
4. Focus on high-frequency, high-impact patterns
