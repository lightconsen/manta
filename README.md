<p align="center">
  <img src="syscity.png" alt="Syscity" width="120" />
</p>
<h1 align="center">Syscity</h1>

<p align="center">
  <a href="https://github.com/lightconsen/syscity/actions/workflows/ci.yml">
    <img src="https://github.com/lightconsen/syscity/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
</p>

Syscity is an AI assistant that runs locally on your machine. Chat with it through a web interface, command line, or connect it to Telegram, Discord, Slack and more.

## Features

- **Web Chat UI** — Built-in responsive chat interface
- **Multi-Channel** — Telegram, Discord, Slack, WhatsApp, Signal, iMessage, QQ, Lark/Feishu
- **Skills System** — Extensible skills with WASM plugin support
- **Agent Teams** — Create teams of agents with roles and hierarchies
- **Memory & Context** — Vector memory, session management, and conversation history
- **Tools** — File operations, web search, shell commands, browser automation, and more
- **Multi-Provider** — OpenAI, DeepSeek, Anthropic, Azure, Ollama, and custom endpoints
- **MCP Support** — Model Context Protocol servers

## Quick Install

### One-line install (Linux / macOS)

```bash
curl -sSL https://syscity.net/install.sh | bash
```

This downloads the latest release binary for your platform and installs it to `/usr/local/bin/syscity`.

### Build from source

```bash
git clone https://github.com/syscity/syscity.git
cd syscity
./build.sh
```

Requires Rust 1.75+ and Node.js (for the web UI).

## Getting Started

### 1. Configure

Run the interactive setup wizard to configure your LLM provider and API key:

```bash
syscity setup
```

You'll be prompted to:
- Select an LLM provider (OpenAI, DeepSeek, Anthropic, etc.)
- Choose a model
- Enter your API key
- Optionally adjust server host/port

The configuration is saved to `~/.syscity/syscity.toml`.

You can also reconfigure later:

```bash
# Edit specific values
syscity config set model=deepseek-chat
syscity config set providers.deepseek.api_key=sk-xxxxx

# Or open the wizard again
syscity config
```

### 2. Start the server

```bash
syscity start
```

This starts the Syscity daemon with the web UI, WebSocket, and API server.

To run in the foreground (useful for debugging):

```bash
syscity start --foreground
```

### 3. Open the Web UI

Open your browser to:

```
http://127.0.0.1:18080
```

The default port is `18080`. If you changed it during setup, use your configured port instead.

### 4. Chat from the command line

```bash
# Interactive chat
syscity chat

# One-shot message
syscity chat --message "What is the weather today?"
```

## Configuration

Configuration is stored in `~/.syscity/syscity.toml`. You can also use environment variables:

```bash
export SYSCITY_API_KEY="your-api-key"
export SYSCITY_BASE_URL="https://api.openai.com/v1"
export SYSCITY_MODEL="gpt-4o"
```

Or set values via CLI:

```bash
syscity config set model=gpt-4o
syscity config set providers.openai.api_key=sk-xxxxx
syscity config show
```

## Running as a Service (Linux)

```bash
# Install systemd service
sudo bash deploy/systemd/install.sh

# Start the service
sudo systemctl start syscity

# Check status
sudo systemctl status syscity
```

## Documentation

- [Architecture](docs/arch.md)
- [Commands](docs/command.md)
- [Channels](docs/modules/channels.md)
- [Skills](docs/modules/skills.md)
- [Providers](docs/modules/providers.md)

## License

MIT
