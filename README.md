# Manta

A lightweight, fast, and secure Personal AI Assistant written in Rust.

## Overview

Manta combines the simplicity philosophy of NanoClaw with the performance characteristics of ZeroClaw:

- **Binary size**: <10MB
- **Memory usage**: <20MB
- **Startup time**: <50ms
- **Single binary**: Easy deployment

## Features

- **Multiple LLM Providers**: OpenAI, Anthropic, Local models (Ollama)
- **Multi-Channel**: Telegram, Discord, Slack, CLI
- **Tool System**: Sandboxed shell, file operations, web search, memory
- **Autonomous Capabilities** (Hermes-Agent inspired):
  - Task planning with todo system
  - Dual memory architecture (procedural + user model)
  - Session search across conversation history
  - Autonomous skill creation
  - Subagent delegation for parallel tasks
  - Context compression
  - Scheduled automation (cron)
- **Secure by Design**: Deny-by-default, explicit allowlists
- **Extensible**: Skill-based capability system

## Quick Start

```bash
# Build
cargo build --release

# Configure
cp config.example.yaml ~/.config/manta/config.yaml
# Edit config.yaml with your API keys

# Run
./target/release/manta
```

## Architecture

See [plan.md](plan.md) for detailed architecture and implementation plan.

## License

MIT
