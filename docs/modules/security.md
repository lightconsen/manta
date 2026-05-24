# Security Module

Input validation, access control, and sandboxing.

## Design

- **Auth** (`auth.rs`) — JWT token generation/validation, API key management
- **Pairing** (`pairing.rs`) — Device pairing and DM policy
- **Sandbox** — Tool-level sandbox via `ToolContext` with `setrlimit` (Unix)
- **Path validation** — `ToolContext::is_path_allowed()` checks workspace boundaries and allowlists
- **Command validation** — `ToolContext::is_command_allowed()` checks against allowlist

### Validators (in `tools/mod.rs`)

- `NameValidator` — Tool name length, character set, prefix rules
- `SchemaValidator` — JSON Schema structure (type: object, properties required)
- `SecurityValidator` — Path traversal and command injection detection

## Missing / TODO

- **Missing**: Full allowlist system with multi-dimensional matching (account, channel, group).
- **Missing**: Rate limiter integration at the gateway level (exists but not fully wired).
- **Missing**: Auth profile store with encrypted credential storage.
- **Missing**: OAuth 2.0 + PKCE implementation.
- **Missing**: `manta doctor` security diagnostic command.
- **Missing**: Audit logging for auth events and tool execution.
- **Missing**: Secret scanning in tool outputs (prevent credential leakage).
