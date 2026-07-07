<p align="center">
  <img src="syscity.png" alt="Syscity" width="120" />
</p>
<h1 align="center">Syscity</h1>
<p align="center"><strong>Agent System</strong></p>

<p align="center">
  <a href="https://github.com/lightconsen/syscity/actions/workflows/ci.yml">
    <img src="https://github.com/lightconsen/syscity/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://github.com/lightconsen/syscity/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License" />
  </a>
  <a href="https://github.com/lightconsen/syscity#requirements">
    <img src="https://img.shields.io/badge/MSRV-1.75-orange.svg" alt="MSRV" />
  </a>
</p>

Syscity is an **agent system** — a runtime that lets AI agents act on your computer. Unlike chatbots that only read and write text, Syscity agents can **control your desktop**, **execute code**, **operate your browser**, and **manage your files**.

Traditional AI lives inside a browser tab. Syscity lives inside your machine.

## What is an Agent System?

An agent system bridges language models with real computing environments:

| Capability | Description |
|---|---|
| Desktop Control | Click, type, scroll, keyboard shortcuts |
| System Automation | AppleScript / shell commands / services |
| Code Execution | Run Python, JavaScript, shell scripts safely |
| Browser Automation | Navigate, click, fill forms, scrape data |
| File Management | Create, edit, move, delete, patch files |
| Web Search | Search the internet for real-time information |

Syscity provides the **action layer**, **memory layer**, and **control plane** that turn a language model into a capable software agent.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Interaction Layer                        │
│  Web UI · Desktop App · CLI · Telegram · Discord · Slack     │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│                      Control Plane (Gateway)                  │
│  Auth · Rate Limiting · WebSocket · ACP Protocol · Webhooks  │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│                      Agent Runtime                            │
│  LLM Routing · Tool Loop · Memory · Sub-Agents (ACP) · MCP  │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│                      Physical Layer                           │
│  Screenshot · Desktop Control · Accessibility · AppleScript  │
│  Shell · File System · Browser · Code Execution · Web Search │
└─────────────────────────────────────────────────────────────┘
```

## Action

- **Desktop Control** — Click, type, scroll, and send keyboard shortcuts (macOS)
- **AppleScript** — Control macOS applications (Mail, Finder, Calendar, etc.)
- **Shell Commands** — Execute bash/zsh commands in a sandboxed environment
- **Code Execution** — Run Python, JavaScript, or shell scripts safely
- **Browser Automation** — Navigate, click, fill forms, and scrape data
- **File Operations** — Create, edit, move, delete, and patch files

## Cognition

- **Multi-Provider LLM** — OpenAI, Anthropic, DeepSeek, Azure, Ollama, and custom endpoints
- **Sub-Agents (ACP)** — Spawn and delegate to sub-agents via the Agent Control Protocol
- **Vector Memory** — Long-term semantic memory with conversation history
- **MCP Support** — Model Context Protocol servers for external tool integration
- **WASM Plugins** — Extend capabilities with sandboxed WebAssembly plugins

## Quick Start

### Install

```bash
# macOS / Linux
curl -sSL https://syscity.net/install.sh | bash
```

See [docs/build.md](docs/build.md) to build from source.

### Configure

```bash
# Interactive setup wizard
syscity setup
```

Config is saved to `~/.syscity/syscity.toml`.

### Start

```bash
# Start the daemon (web UI + API + WebSocket)
syscity start

# Or run in the foreground
syscity start --foreground
```

Open `http://127.0.0.1:18080` for the Web UI.

### Agent in Action

Open the Web UI, or attach the terminal client:

```bash
# Interactive terminal UI (connects to the running daemon)
syscity tui
```

Then ask the agent something like *"Take a screenshot and tell me what's on
my screen"*. The agent can:

- Capture your screen
- Read the UI tree of frontmost windows
- Click buttons or type text
- Execute AppleScript to control apps
- Run shell commands and return results

See the [Getting Started guide](docs/getting-started.md) for a full walkthrough.

## macOS Desktop Control (Best Experience)

On macOS, Syscity unlocks the full desktop automation stack:

| Tool | What it does |
|---|---|
| `macos_screenshot` | Capture full screen, window, or region |
| `macos_accessibility` | Read UI tree of any application |
| `macos_desktop_control` | Click, type, scroll, keyboard shortcuts |
| `applescript` | Control Mail, Calendar, Finder, Music, etc. |

Grant **Screen Recording** and **Accessibility** permissions in System Settings for full capability.

## Configuration

```bash
# Set LLM provider and key
syscity config set providers.openai.api_key=sk-xxxxx
syscity config set model=gpt-4o

# Or use environment variables
export SYSCITY_API_KEY="your-api-key"
export SYSCITY_MODEL="gpt-4o"
```

## Documentation

- [Getting Started](docs/getting-started.md)
- [Build from Source](docs/build.md)
- [Architecture](docs/arch.md)
- [OS Capability Architecture](docs/os.md)
- [Protocol](docs/protocol.md)
- [Slash Commands](docs/command.md)
- [Full documentation index](docs/README.md)

## License

Apache-2.0
