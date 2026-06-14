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

## Implemented Features

- Auth mode ambiguity detection — fails fast when both `shared_token` and OAuth are configured but `auth_mode` is not set. See `src/gateway/mod.rs:validate_auth_config()`.
- Tailscale authentication — `TailscaleAuthenticator` with whois verification via CLI, caching, and tailnet-based authorization. See `src/security/tailscale.rs`.
- Full allowlist system with multi-dimensional matching — 10 match sources (Id, Username, Name, Tag, E164, PrefixedId, PrefixedUser, PrefixedName, Slug, Localpart), compiled `HashSet` cache, wildcard `*` support, account-scoped storage with file locking. Wired into `PairingStore::is_authorized()`. See `src/security/allowlist.rs`.
- SecretRef resolution system — three providers (`env`, `file`, `exec`) with `SecretResolver::resolve()`. See `src/secrets.rs:85-298`.
- Multi-scope rate limiting — `MultiTierRateLimiter` includes `global`/`per_user`/`per_ip`/`per_endpoint`/`shared_secret`/`device_token`/`hook_auth`/`control_plane_write` tiers, lockout tracking, attempt serialization, and loopback exemption. See `src/gateway/rate_limit.rs` and `src/security/sliding_window.rs`.
- Device pairing challenge — `DevicePairingStore` generates 8-character unambiguous codes, enforces a max pending limit, produces QR codes and `syscity://pair/{code}` URIs, and supports base64url setup URLs (`/api/v1/device/pairing/setup/{token}`). See `src/security/device_pairing.rs` and `src/gateway/handlers/device_pairing.rs`.
- Secret resolution cache — `SecretResolver` keeps per-provider payload caches (env, file, exec) and a per-reference result cache with TTL, plus `refresh()` and `refresh_reference()` for manual invalidation. See `src/secrets.rs`.
- Audit logging for auth events — `AuditEventType` includes `Login`, `Logout`, and `TokenValidation`; `AuthManager` emits events via an attached `AuditLogger` on `create_session`, `validate_session`, and `revoke_session`; gateway `auth_middleware` emits `TokenValidation` for missing/malformed headers. See `src/security/runtime_audit.rs`, `src/security/mod.rs`, and `src/gateway/middleware.rs`.
- Trusted proxy authentication — IP whitelist/CIDR, required headers, user extraction, allowUsers whitelist, audit logging. See `src/security/trusted_proxy.rs` and `src/gateway/middleware.rs::trusted_proxy_auth_middleware`.
- Credential precedence — `env_first` vs `config_first` for token/password sources. `CredentialPrecedence` enum in `src/gateway/mod.rs:SecurityConfig`, applied in `src/daemon.rs:apply_env_security_overrides()` and `apply_env_provider_overrides()`. Default is `env_first`; `config_first` keeps config-file credentials when both env and file are present.
- Secret scanning in tool outputs — `ContentFilter` with `SecretScanner` and `PiiDetector` wired into `ToolRegistry` during `Gateway::new()` via `create_default_tool_registry()`. See `src/security/content_filter.rs` and `src/tools/mod.rs:ToolRegistry::execute()`.
