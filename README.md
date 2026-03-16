# Manta

A lightweight, fast, and secure application written in Rust with clean architecture.

## Overview

Manta demonstrates modern Rust development practices with a layered architecture:

- **Clean Architecture**: Core domain logic independent of external frameworks
- **Layered Design**: Clear separation between core, adapters, and interfaces
- **Async/Await**: Modern async Rust patterns
- **Error Handling**: Structured error types with context
- **Configuration**: Multi-source configuration (files, environment)
- **Observability**: Structured logging with tracing
- **CLI**: Comprehensive command-line interface

## Quick Start

```bash
# Build
cargo build --release

# Configure (optional)
# Manta works out of the box with defaults

# Run CLI
./target/release/manta --help

# Start server
./target/release/manta server

# Create and manage entities
./target/release/manta entity create "My Entity"
./target/release/manta entity list
```

## Architecture

Manta follows clean architecture principles:

```
manta/
├── src/
│   ├── core/           # Domain logic (independent)
│   │   ├── models.rs   # Domain models (Entity, Status, etc.)
│   │   └── engine.rs   # Business logic
│   ├── adapters/       # External integrations
│   │   ├── storage.rs  # Storage implementations
│   │   └── api.rs      # HTTP client with retry logic
│   ├── config.rs       # Configuration management
│   ├── cli.rs          # Command-line interface
│   ├── error.rs        # Error types
│   └── utils/          # Utilities
│       └── logging.rs  # Logging setup
├── tests/              # Integration tests
└── CLAUDE.md           # Development guide
```

## Features

### Implemented

- ✅ Clean architecture with separation of concerns
- ✅ Entity management (create, read, update, delete)
- ✅ Configuration system (file, env, CLI)
- ✅ Structured logging with tracing
- ✅ CLI with subcommands
- ✅ In-memory and file-based storage
- ✅ HTTP client with retry logic
- ✅ Comprehensive error handling
- ✅ Unit and integration tests

### Planned

- Multiple LLM Providers (OpenAI, Anthropic, Local)
- Multi-Channel support (Telegram, Discord, CLI)
- Tool system for autonomous operations
- Memory management

See [plan.md](plan.md) for detailed architecture and roadmap.

## Configuration

Manta can be configured via:

1. **Configuration file**: `manta.toml` or `~/.manta/manta.toml`
2. **Environment variables**: `MANTA_SERVER_HOST`, `MANTA_LOG_LEVEL`, etc.
3. **Command-line flags**: `--config`, `--log-level`

### Example Configuration

```toml
[server]
host = "127.0.0.1"
port = 8080

[logging]
level = "info"
format = "compact"  # Options: compact, pretty, json

[storage]
type = "memory"  # Options: memory, file
```

## Development

### Prerequisites

- Rust 1.75 or later
- Cargo

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with logging
cargo run -- --log-level debug server
```

### Code Quality

```bash
# Format code
cargo fmt

# Run linter
cargo clippy -- -D warnings

# Generate documentation
cargo doc --no-deps
```

See [CLAUDE.md](CLAUDE.md) for Rust best practices and development guidelines.

## License

MIT OR Apache-2.0
