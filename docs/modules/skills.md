# Skills Module

OpenClaw-compatible skill system for extending agent behavior with modular, trigger-activated prompt bundles.

## Design

A **skill** is a reusable prompt bundle with YAML frontmatter metadata, trigger conditions, runtime requirements, and optional dependencies. Skills are stored at multiple levels and can be hot-reloaded.

- **`Skill`** — Core skill struct: name, description, prompt, triggers, metadata, dependencies
- **`SkillRegistry`** — Remote registry client for discovery and installation (ClawHub, skills.manta.dev)
- **`SkillStorage`** / **`StorageLevel`** — Multi-level storage:
  - `Bundled` — Built into the binary
  - `User` — `~/.manta/skills/`
  - `Workspace` — `{workspace}/skills/`
  - `Project` — `{project}/.manta/skills/`
- **`SkillWatcher`** — File system watcher for hot reload
- **`SkillFrontmatter`** — YAML frontmatter parser for `SKILL.md` format
- **`DependencyGraph`** — Resolves skill dependency chains before activation

### Trigger Types

| Type | Activation |
|------|------------|
| `Regex` | Pattern match on user input |
| `Keyword` | Substring match |
| `Command` | Slash command (e.g., `/weather`) |
| `Intent` | Intent classification match |

### Runtime Requirements (`SkillRequires`)

- Required binaries on PATH
- Required environment variables
- Required config file paths
- Supported operating systems

### Trust Levels

Skills carry a `SkillTrust` level (Community or Trusted). Community skills restrict the agent to read-only (non-privileged) tools so mixing a community skill with a trusted one does not escalate privileges.

### Built-in Skills

`builtin.rs` provides skills that are always available:
- `skill-creator` — Create and package new skills
- `find-skills` — Discover available skills
- `cron` — Schedule recurring tasks
- `clawhub` — Browse ClawHub skill registry
- `summarize` — Summarize conversations
- `weather` — Weather queries
- `tmux` — Tmux integration
- `github` — GitHub operations
- `agent-browser` — Web browsing
- `api-gateway` — API gateway management
- `nano-pdf` — PDF operations
- `self-improving-agent` — Agent self-improvement
- `agent-creator` — Create specialized agents

## Key Types

```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,
    pub triggers: Vec<SkillTrigger>,
    pub prompt: String,
    pub metadata: OpenClawMetadata,
    pub depends_on: HashMap<String, String>,
    pub source_level: StorageLevel,
    pub is_eligible: bool,
    pub enabled: bool,
}

pub struct SkillTrigger {
    pub trigger_type: TriggerType,
    pub pattern: String,
    pub priority: i32,
    pub user_invocable: bool,
    pub model_invocable: bool,
}
```

## Missing / TODO

- **Missing**: Skill activation in the agent prompt builder — skills are parsed and stored but not dynamically injected into system prompts during conversation.
- **Missing**: Remote skill installation from registry (`install.rs` exists but integration with agent is not complete).
- **Missing**: Skill dependency chain resolution at runtime (`DependencyGraph` exists but not wired into agent startup).
- **Missing**: Slash command integration with channels — command triggers are defined but not wired to channel message handlers.
- **Missing**: Skill file watcher hot reload — `SkillWatcher` exists but is not started in `Gateway::start()`.
- **Missing**: Token optimization (path compaction, size limits) for skill prompts.
- **Missing**: Skill version resolution and semver compatibility enforcement.
