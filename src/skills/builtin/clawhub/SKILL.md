---
name: clawhub
description: "Browse, install, and share skills from the ClaW Hub community registry"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "clawhub"
    priority: 100
  - type: keyword
    pattern: "install skill"
    priority: 90
  - type: keyword
    pattern: "find skill"
    priority: 80
openclaw:
  emoji: "🦞"
  category: "meta"
  tags:
    - "skills"
    - "registry"
    - "community"
---

# ClaW Hub Skill

Browse and install community skills from the ClaW Hub registry.

## Capabilities

- Search the skill registry by name, category, or keyword
- Install skills directly into the local skills directory
- Publish skills to the community registry
- Update installed skills to latest versions
- View skill details, ratings, and documentation

## Usage Examples

### Search for skills
"Find skills for web scraping" or "Search clawhub for database skills"

### Install a skill
"Install the postgres skill from clawhub" or "clawhub install weather-advanced"

### List installed skills
"What skills do I have installed?" or "Show my clawhub skills"

### Publish a skill
"Publish my custom skill to clawhub" or "Share this skill with the community"

## Skill Metadata

Each skill in the registry includes:
- Name and description
- Version and author
- Download count and rating
- Required dependencies
- Compatible platforms

## Best Practices

1. Review skill source before installing
2. Check ratings and download counts for quality signals
3. Pin skill versions for reproducible deployments
4. Contribute skills back to the community
