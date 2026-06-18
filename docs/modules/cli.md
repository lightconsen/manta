# CLI Module

Command-line interface for Syscity using the `clap` crate.

## Design

- **`Cli`** — Root CLI parser with global flags (`--config`, `--log-level`)
- **Subcommand modules** — Each major subsystem has its own CLI subcommand:
  - `admin` — Gateway management (provider switching, status)
  - `agent` — Agent personality management
  - `approval` — Approval queue management
  - `audit` — Audit log queries
  - `capability` — Capability set management
  - `channel` — Channel management (Telegram, Discord, Slack)
  - `config_cmd` — Configuration management (get, set, validate)
  - `cron` — Cron job management
  - `daemon` — Daemon lifecycle (start, stop, status, logs)
  - `device` — Device pairing management
  - `doctor` — Diagnostic system with hints
  - `entity` — Entity management
  - `export` — Export conversations and memories
  - `mcp` — MCP server management
  - `memory` — Vector memory management
  - `plugin` — Plugin management for WASM extensions
  - `provider` — LLM provider management
  - `security` — Security commands (gate, pairing)
  - `session` — Session management
  - `setup` — Initial setup wizard
  - `skill` — Skill management
  - `device` — Device pairing management
  - `approval` — Approval queue management
  - `audit` — Audit log queries
  - `provider` — LLM provider management
  - `doctor` — Diagnostic system with hints
  - `capabilities` — Check available OS capability sets
  - `tui` — Interactive terminal UI client

### Top-level Commands

| Command | Description |
|---------|-------------|
| `start` | Start the Syscity daemon |
| `stop` | Stop the Syscity daemon |
| `reload` | Reload plugins and configuration |
| `status` | Check daemon status |
| `logs` | Show and tail daemon logs |
| `health` | Health check |
| `assistant-run` | Run as an assistant process (internal) |
| `capabilities` | Check and display available OS capability sets |
| `tui` | Interactive terminal UI client |

## Key Types

```rust
#[derive(Debug, Parser)]
#[command(name = "syscity")]
pub struct Cli {
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,
    #[arg(short, long, global = true)]
    pub log_level: Option<String>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Entity { command: EntityCommands },
    Export { command: ExportCommands },
    Config { command: Option<ConfigCommands> },
    Health,
    AssistantRun { config: PathBuf },
    Admin { command: AdminCommands },
    Cron { command: CronCommands },
    Skill { command: SkillCommands },
    Agent { command: AgentCommands },
    Channel { command: ChannelCommands },
    Plugin { command: PluginCommands },
    Start { host: String, port: u16, foreground: bool, ... },
    Stop { force: bool },
    Reload,
    Status,
    Logs { lines: usize, follow: bool },
    Mcp { command: McpCommands },
    Memory { command: MemoryCommands },
    Provider { command: ProviderCommands },
    Security { command: SecurityCommands },
    Session { command: SessionCommands },
    Device { command: DeviceCommands },
    Approval { command: ApprovalCommands },
    Audit { command: AuditCommands },
    Doctor { command: DoctorCommands },
    Setup,
    Provider { command: ProviderCommands },
    Capabilities,
    Tui { host: String, port: u16, token: Option<String>, session: Option<String> },
}
```

## Implemented Features

- Comprehensive CLI with 20+ subcommand modules
- Global `--config` and `--log-level` flags
- Daemon lifecycle management (start, stop, reload, status, logs)
- Provider management with auth and OAuth commands
- Channel management for all supported platforms
- Plugin and skill management
- Memory search and management
- MCP server configuration
- Security commands (pairing, gate, audit)
- Diagnostic system with `doctor` command
- Session and entity management
- Export functionality for conversations and memories
- Device pairing commands
- Approval queue management

