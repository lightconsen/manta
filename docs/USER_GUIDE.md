# Manta User Guide

Welcome to Manta, your personal AI assistant! This guide will help you get started and make the most of Manta's capabilities.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Using the CLI](#using-the-cli)
- [Tools](#tools)
- [Skills](#skills)
- [Memory](#memory)
- [Security](#security)
- [Troubleshooting](#troubleshooting)

## Installation

### From Source

```bash
# Clone repository
git clone https://github.com/anthropics/manta
cd manta

# Build release binary
cargo build --release

# Install to /usr/local/bin
sudo cp target/release/manta /usr/local/bin/
```

### Docker

```bash
docker-compose up -d
```

### Systemd Service

```bash
cd deploy/systemd
sudo ./install.sh
```

## Quick Start

1. **Set up environment variables:**

```bash
export MANTA_BASE_URL="https://api.openai.com/v1"
export MANTA_API_KEY="your-api-key"
export MANTA_MODEL="gpt-4o-mini"
```

2. **Start chatting:**

```bash
manta chat
```

3. **Send a single message:**

```bash
manta chat -m "Hello, Manta!"
```

## Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `MANTA_BASE_URL` | LLM API endpoint | Yes |
| `MANTA_API_KEY` | Your API key | Yes |
| `MANTA_MODEL` | Model name (default: gpt-4o-mini) | No |
| `MANTA_IS_ANTHROPIC` | Use Anthropic format (default: false) | No |
| `MANTA_AGENT_NAME` | Assistant name (default: Manta) | No |

### Config File

Create `~/.manta/config.yaml`:

```yaml
provider:
  type: openai
  model: gpt-4o-mini
  temperature: 0.7

agent:
  name: Manta
  system_prompt: |
    You are a helpful AI assistant.

features:
  skills: true
  cron: true
  memory: true

security:
  allow_shell: true
  sandboxed: true
  max_budget: 50
```

## Using the CLI

### Interactive Mode

```bash
manta chat
```

Special commands in interactive mode:
- `/help` - Show help
- `/exit` or `/quit` - Exit
- `/clear` - Clear screen
- `/tools` - List available tools
- `/reset` - Reset conversation

### Single Message

```bash
manta chat -m "What's the weather today?"
```

### Working Directory

Manta can work with files in a specific directory:

```bash
manta chat --work-dir ./my-project
```

## Tools

Manta has access to various tools to help you:

### File Operations

- **read_file** - Read file contents
- **write_file** - Create or overwrite files
- **edit_file** - Make targeted edits
- **glob** - Search for files

Example:
```
You: Read the contents of src/main.rs
Manta: I'll read that file for you.
[Tool: read_file]
```

### Web Tools

- **web_search** - Search the internet
- **web_fetch** - Fetch page content

Example:
```
You: Search for Rust async best practices
Manta: Let me search for that.
[Tool: web_search]
```

### Shell

- **shell** - Execute commands

Example:
```
You: Run cargo test
Manta: I'll run the tests.
[Tool: shell]
```

### Task Management

- **todo** - Manage your tasks

Example:
```
You: Add todo: Review pull request #42
Manta: Added todo: Review pull request #42
```

### Code Execution

- **execute_code** - Run Python code safely

Example:
```
You: Calculate 15 * 23 + 7
Manta: I'll calculate that using Python.
[Tool: execute_code]
Result: 352
```

## Skills

Skills extend Manta's capabilities with specialized behaviors.

### Built-in Skills

Manta comes with example skills:
- **weather** - Get weather information
- **news** - Fetch news headlines
- **calculator** - Mathematical calculations
- **todo** - Task management
- **reminder** - Set reminders

### Loading Skills

Skills are loaded from `~/.manta/skills/`.

To install example skills:

```bash
cp -r examples/skills/* ~/.manta/skills/
```

### Creating Custom Skills

Create a directory with a `SKILL.md` file:

```markdown
# My Skill

## Triggers

- Keyword: "mykeyword"
- Regex: pattern

## Prompt

When triggered, do this specific thing...

## Example Usage

User: "mykeyword test"
Response: "Handled!"
```

## Memory

Manta has a dual memory system:

### Procedural Memory (agent.md)

Store instructions for how Manta should behave:

```markdown
# How I Work

## Communication Style
- Be concise
- Use examples

## Common Tasks
### Code Review
1. Check formatting
2. Verify tests
3. Review logic
```

### User Model (user.md)

Store information about you:

```markdown
# User Profile

## Preferences
- Language: Rust, Python
- Editor: VS Code
- Timezone: PST

## Projects
- Working on: Manta AI
```

### Session Search

Search through past conversations:

```
You: Search sessions for "docker setup"
Manta: Found 3 relevant sessions...
```

## Security

### Allowlist

Control who can use Manta:

```yaml
security:
  allowlist:
    enabled: true
    users:
      - "user1@example.com"
      - "user2@example.com"
```

### Rate Limiting

Prevent abuse:

```yaml
security:
  rate_limit:
    requests_per_minute: 30
    requests_per_hour: 500
```

### Sandboxing

Code execution is sandboxed:
- 5-minute timeout
- 50KB output limit
- Network restrictions
- Forbidden imports blocked

## Troubleshooting

### Connection Issues

```bash
# Test API connectivity
curl $MANTA_BASE_URL/models \
  -H "Authorization: Bearer $MANTA_API_KEY"
```

### Check Logs

```bash
# Systemd
sudo journalctl -u manta -f

# Docker
docker-compose logs -f manta
```

### Reset Everything

```bash
# Clear data
rm -rf ~/.local/share/manta/*
rm -rf ~/.manta/memory/*
```

### Debug Mode

```bash
RUST_LOG=debug manta chat
```

### Common Issues

**"Invalid API key"**
- Check `MANTA_API_KEY` is set correctly
- Verify key hasn't expired

**"Model not found"**
- Verify `MANTA_MODEL` is available at your provider
- Check `MANTA_BASE_URL` is correct

**"Permission denied"**
- Check file permissions in working directory
- Verify `MANTA_ALLOW_SHELL` is set if using shell

## Tips

1. **Be specific** - Clear instructions get better results
2. **Provide context** - Mention relevant files or previous conversations
3. **Iterate** - Refine requests based on responses
4. **Use skills** - Create skills for repetitive tasks
5. **Review memory** - Keep agent.md and user.md updated

## Getting Help

- GitHub Issues: https://github.com/anthropics/manta/issues
- Documentation: https://docs.manta.dev
- Community: https://discord.gg/manta
