# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Release workflow integration: the `## [X.Y.Z]` section matching a pushed tag
becomes the GitHub Release body. Write release notes here BEFORE tagging;
if no section matches, the release falls back to auto-generated notes.

## [Unreleased]

## [0.2.2] - 2026-08-24

### Highlights

- **Kernel write fences on all three desktop platforms**: `workspace_only` command tools (shell / process / code_exec) now run inside macOS Seatbelt, Linux Landlock, and Windows AppContainer + Job Object sandboxes — the agent can no longer write outside the workspace, enforced by the OS kernel, fail-closed.
- **Fresh-context goal loops ("Ralph" mode)**: `/goal --fresh` runs each round in a brand-new seedless sub-agent with a validated `handoff` JSON contract; round notes are browsable in the workspace, and interrupted goals are suspended with a structured `blocked_reason` until `/goal resume`.
- **Human-in-the-loop `ask_user` tool** with a web modal — agents can ask blocking questions mid-turn.
- **Per-turn observability**: full-fidelity turn records under `~/.syscity/turns/` plus SQLite metrics and a `syscity observe {stats,list,show,export,prune}` CLI.
- **Online self-update** for CLI, daemon, and desktop (minisign-signed updater bundles; verify-then-apply with SHA-256).

### Added

- Claude-Code-compatible shell hooks (`~/.syscity/hooks.json`): PreToolUse / PostToolUse / UserPromptSubmit / Stop with deny/ask/block decisions, fail-open by design
- Runtime invariant registry: modules register their own checks, `syscity invariants [--json]` runs them all, enforced by static analysis
- Content-addressed attachment store (`~/.syscity/attachments/sha256/…`) with dedup and `observe prune` GC
- Report artifacts now live in each agent's own workspace, served at `/api/v1/artifacts/@<agent>/…`
- Agent workspace file browser in the web UI (`workspace.list` / `workspace.read`)
- `screen_ui_detect` tool (OmniParser) with automatic fallback when the a11y tree is empty
- Post-execute tool hooks with block-with-feedback
- Cron run digest appended to `workspace/cron-log.md`
- Durable compaction: boundaries persisted, tool pairs kept, overflow retries once
- Crash-recovery: orphaned in-flight tasks marked failed at startup; session repair with `TOOL_OUTCOME_UNKNOWN` sentinel
- Eval framework in CI: deterministic YAML validation on every push, nightly `ci_smoke` smoke run against a live LLM (non-blocking, opens a tracking issue on failure), manual `release_gate` before tagging

### Changed

- Todo tool is now whole-snapshot replace (`{"todos": [...]}`); plans clear automatically on each new turn
- File writes guarded by read-before-edit version tracking
- Oversized tool outputs spill to disk (path-aware exemptions), shell keeps output tails
- Current time moved out of the system prompt into a per-request state snapshot (keeps the prompt prefix byte-stable for KV-cache reuse)
- Secrets masked on all config surfaces; `config.set` uses revision CAS (`REVISION_CONFLICT` on stale writes)
- `web_fetch`: stable error codes, per-hop redirect revalidation, non-2xx returned as result
- Unix process spawns report the terminating signal and kill the whole process group
- Grounding & honesty rules in the default system prompt: cite only tool-result facts, surface source conflicts, label prior knowledge as unverified

### Fixed

- Windows builds: gated Unix-only code paths; verified continuously via the zig cross-compile harness
- LLM judge hardened for evals: JSON recovery from prose, format-correction retry, declared-dimension gating, normalized threshold keys
- E2E tests allocate ports dynamically (no more "Failed to bind gateway" flakes)
- Desktop release pipeline: macOS deployment target pinned for llama.cpp, updater signing key rotated, updater manifests generated for tauri v2, desktop bundles collected from the workspace-root target dir

## [0.1.2] - 2026-06-11

### Added

- Initial release of Syscity AI Assistant
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

### Changed

- Relicense project from MIT to Apache-2.0

## [0.1.1] - 2026-06-04

### Changed

- Upgrade `sqlx` 0.7 -> 0.8.6 (security fix)
- Upgrade `wasmtime` 15 -> 45.0.0 (security fix)
- Remove `rustls-pemfile` and `rsa` from dependency tree

## [0.1.0] - 2024-01-01

### Added
- Initial project setup
- Basic structure and CI
