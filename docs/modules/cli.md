# CLI Module

Command-line interface for Syscity using the `clap` crate.

## Design

- **`Cli`** — Root CLI parser with global flags (`--config`, `--log-level`)
- **Subcommand modules** — Each major subsystem has its own CLI subcommand:
  - `admin` — Gateway management (provider switching, status)
  - `agent` — Agent personality management
  - `approval` — Approval queue management
  - `audit` — Audit log queries
  - `auth` — Auth token lifecycle and mode management
  - `capability` — Capability set management
  - `channel` — Channel management (Telegram, Discord, Slack)
  - `config_cmd` — Configuration management (get, set, validate)
  - `cron` — Cron job management
  - `daemon` — Daemon lifecycle (start, stop, status, logs)
  - `device` — Device pairing management
  - `doctor` — Diagnostic system with hints
  - `entity` — Entity management
  - `eval` — Evaluation suites (list, validate, run)
  - `export` — Export conversations and memories
  - `kb` — Knowledge base management (ingest, list, delete)
  - `mcp` — MCP server management
  - `memory` — Vector memory management
  - `observe` — Per-turn observability records (stats, list, show, export, prune)
  - `plugin` — Plugin management for WASM extensions
  - `provider` — LLM provider management
  - `secrets` — Secret store management (list, migrate, purge)
  - `security` — Security commands (gate, pairing)
  - `session` — Session management
  - `setup` — Initial setup wizard
  - `skill` — Skill management
  - `tui` — Interactive terminal UI client
  - `update` — Self-update from GitHub Releases
  - `capabilities` — Check available OS capability sets

### Top-level Commands

| Command | Description |
|---------|-------------|
| `start` | Start the Syscity daemon |
| `stop` | Stop the Syscity daemon |
| `reload` | Reload plugins and configuration |
| `restart` | Restart the daemon (used internally by the self-update helper) |
| `status` | Check daemon status |
| `logs` | Show and tail daemon logs |
| `health` | Health check |
| `invariants [--json]` | Run all registered runtime invariant checks against live local state; exits non-zero if any invariant is violated (CI/cron-friendly). `--json` emits a machine-readable report instead of a table |
| `assistant-run` | Run as an assistant process (internal) |
| `capabilities` | Check and display available OS capability sets |
| `tui` | Interactive terminal UI client |

`invariants` runs the module-owned checks registered in the runtime invariant
registry (`src/core/invariants.rs`): every top-level module either registers
its data-guarantee checks or carries an explicit `INVARIANTS-NONE:` marker
(enforced by `scripts/static-analysis.sh`). Checks that find no state on disk
(e.g. no store yet) count as passed with a "not applicable" note.

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
    Eval { command: EvalCommands },
    Export { command: ExportCommands },
    Config { command: Option<ConfigCommands> },
    Health,
    Invariants { json: bool },
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
    Restart { pid: Option<u32>, host: String, port: u16 },
    Logs { lines: usize, follow: bool },
    Mcp { command: McpCommands },
    Memory { command: MemoryCommands },
    Kb { command: KbCommands },
    Security { command: SecurityCommands },
    Session { command: SessionCommands },
    Observe { command: ObserveCommands },
    Update { command: Option<UpdateCommands> },
    Secrets { command: SecretsCommands },
    Setup,
    Device { command: DeviceCommands },
    Approval { command: ApprovalCommands },
    Audit { command: AuditCommands },
    Auth { command: AuthCommands },
    Provider { command: ProviderCommands },
    Doctor { command: DoctorCommands },
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
- Runtime invariant checks via `invariants` (module-owned data guarantees)
- Self-update via `update` (CLI, daemon, desktop)
- Auth token lifecycle management via `auth`
- Session and entity management
- Export functionality for conversations and memories
- Device pairing commands
- Approval queue management

