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

- **✅ Implemented**: Auth mode ambiguity detection — fails fast when both `shared_token` and OAuth are configured but `auth_mode` is not set. See `src/gateway/mod.rs:validate_auth_config()`.
- **✅ Implemented**: Tailscale authentication — `TailscaleAuthenticator` with whois verification via CLI, caching, and tailnet-based authorization. See `src/security/tailscale.rs`.
- **✅ Implemented**: Full allowlist system with multi-dimensional matching — 10 match sources (Id, Username, Name, Tag, E164, PrefixedId, PrefixedUser, PrefixedName, Slug, Localpart), compiled `HashSet` cache, wildcard `*` support, account-scoped storage with file locking. Wired into `PairingStore::is_authorized()`. See `src/security/allowlist.rs`.
- **✅ Implemented**: SecretRef resolution system — three providers (`env`, `file`, `exec`) with `SecretResolver::resolve()`. See `src/secrets.rs:85-298`. Missing advanced features: env allowlist, JSON Pointer for file, JSON protocol for exec, batch resolution.
- **📝 Partial**: Multi-scope rate limiting — `MultiTierRateLimiter` with `global`/`per_user`/`per_ip`/`per_endpoint` tiers and sliding window algorithm exist (`src/gateway/rate_limit.rs:82-95`, `src/security/sliding_window.rs:108-120`). Missing: auth-specific scopes (shared_secret, device_token, hook_auth), lockout, attempt serialization, control-plane write limiter, loopback exemption.
- **📝 Partial**: Device pairing challenge — `DevicePairingStore` with 5-character unambiguous codes, 1h TTL exists (`src/security/device_pairing.rs:67-88`). Missing: 8-character codes, max pending limit, QR-code pairing, setup code with base64url URL.
- **📝 Partial**: Secret resolution cache — `SecretsSnapshot` with TTL and degraded-mode fallback exists (`src/secrets.rs:182-237`). Missing: per-provider payload cache, per-ref result cache, manual refresh.
- **📝 Partial**: Audit logging for auth events and tool execution — `RuntimeAuditLog` / `PersistentAuditLog` record `ToolInvocation`, `ToolDeny`, `AccessCheck`, `Security`, and pairing events (`src/security/runtime_audit.rs:16-43`). Missing: fine-grained auth events (login, logout, token validation).
- **✅ Implemented**: Trusted proxy authentication — IP whitelist/CIDR, required headers, user extraction, allowUsers whitelist, audit logging. See `src/security/trusted_proxy.rs` and `src/gateway/middleware.rs::trusted_proxy_auth_middleware`.
- **Missing**: Credential precedence — `env-first` vs `config-first` for token/password sources.
- **Missing**: Secret scanning in tool outputs (prevent credential leakage).
