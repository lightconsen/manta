# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of Manta AI Assistant
- Core agent architecture with tool system
- SQLite persistence for sessions and memory
- Provider abstraction supporting OpenAI and Anthropic APIs
- CLI with interactive chat mode
- Web search and fetch tools
- File operations (read, write, edit, glob)
- Shell command execution
- Code execution with Python sandbox
- Todo/task management
- Session search with FTS5
- Dual memory architecture (procedural + user model)
- Context compression strategies
- Iteration budget management
- Autonomous skill creation with security guard
- Subagent delegation
- Persistent assistant spawning
- Assistant mesh for inter-assistant communication
- MCP (Model Context Protocol) integration
- Security module with auth, allowlist, and rate limiting
- Cron scheduler for recurring tasks
- Telegram channel integration
- Discord channel integration
- Slack channel integration
- Message formatting for all channels
- Docker deployment configuration
- Systemd service configuration
- Kubernetes manifests
- GitHub Actions CI/CD workflows
- Example skills (weather, news, calculator, todo, reminder)
- Comprehensive documentation

## [0.1.0] - 2024-01-01

### Added
- Initial project setup
- Basic structure and CI
