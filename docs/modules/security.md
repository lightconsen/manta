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

- **Missing**: Full allowlist system with multi-dimensional matching — 10+ match sources (id, username, name, tag, e164, prefixed-id, prefixed-user, prefixed-name, slug, localpart), compiled `HashSet` cache, wildcard `*` support, account-scoped storage with file locking.
- **Missing**: Auth mode ambiguity detection — when token and password both configured but mode unset, assert explicit mode selection at startup / `manta doctor`.
- **Missing**: SecretRef resolution system — three providers (`env` with optional allowlist, `file` with path security assertions and JSON Pointer, `exec` with stdin/stdout JSON protocol), batch resolution with concurrency limits, per-provider caching.
- **Missing**: Multi-scope rate limiting — auth-specific scopes (default, shared_secret, device_token, hook_auth) with sliding window + lockout, attempt serialization anti-race (`withSerializedAttempt`), control-plane write limiter (fixed window), loopback exemption.
- **Missing**: Device pairing challenge — 8-character code excluding `0/O/1/I`, TTL (1h), max pending (3), store-backed allowlist merge, setup code (base64url URL + bootstrap token), QR-code pairing.
- **Missing**: Tailscale authentication — `tailscale whois` reverse lookup verification.
- **Missing**: Trusted proxy authentication — IP whitelist, required headers, user extraction, allowUsers whitelist.
- **Missing**: Credential precedence — `env-first` vs `config-first` for token/password sources.
- **Missing**: Secret resolution cache — per-provider payload cache, per-ref result cache, TTL and manual refresh.
- **Missing**: Audit logging for auth events and tool execution.
- **Missing**: Secret scanning in tool outputs (prevent credential leakage).
