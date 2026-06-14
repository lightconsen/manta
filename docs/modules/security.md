# Security Module

Input validation, access control, authentication, and sandboxing.

## Design

- **Auth** (`auth.rs`) — JWT token generation/validation, API key management, session management with scopes
- **Pairing** (`pairing.rs`) — Device pairing and DM policy
- **Device Pairing** (`device_pairing.rs`) — Challenge-based device pairing with QR codes and setup URLs
- **Allowlist** (`allowlist.rs`) — Multi-dimensional allowlist matching with compiled cache
- **Tailscale Auth** (`tailscale.rs`) — Tailscale whois verification with caching
- **Trusted Proxy** (`trusted_proxy.rs`) — IP whitelist/CIDR, required headers, user extraction
- **Runtime Audit** (`runtime_audit.rs`) — Audit logging for auth and security events
- **Content Filter** (`content_filter.rs`) — Secret scanning and PII detection in outputs
- **Sliding Window** (`sliding_window.rs`) — Rate limit tracking with lockout

### Validators (in `tools/mod.rs`)

- `NameValidator` — Tool name length, character set, prefix rules
- `SchemaValidator` — JSON Schema structure (type: object, properties required)
- `SecurityValidator` — Path traversal and command injection detection

## Key Types

```rust
pub struct AuthManager {
    users: Arc<RwLock<HashMap<UserId, User>>>,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    pairing_required: bool,
    audit_log: Option<Arc<dyn AuditLogger>>,
}

pub struct User {
    pub id: UserId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_admin: bool,
    pub scopes: Vec<String>,
    pub metadata: HashMap<String, String>,
}

pub struct Session {
    pub token: String,
    pub user_id: UserId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub device_fingerprint: Option<String>,
    pub scopes: Vec<String>,
}
```

## Implemented Features

- Auth mode ambiguity detection — fails fast when both `shared_token` and OAuth are configured but `auth_mode` is not set
- Tailscale authentication — `TailscaleAuthenticator` with whois verification via CLI, caching, and tailnet-based authorization
- Full allowlist system with multi-dimensional matching — 10 match sources (Id, Username, Name, Tag, E164, PrefixedId, PrefixedUser, PrefixedName, Slug, Localpart), compiled `HashSet` cache, wildcard `*` support, account-scoped storage with file locking
- SecretRef resolution system — three providers (`env`, `file`, `exec`) with `SecretResolver::resolve()`
- Multi-scope rate limiting — `MultiTierRateLimiter` includes `global`/`per_user`/`per_ip`/`per_endpoint`/`shared_secret`/`device_token`/`hook_auth`/`control_plane_write` tiers, lockout tracking, attempt serialization, and loopback exemption
- Device pairing challenge — `DevicePairingStore` generates 8-character unambiguous codes, enforces a max pending limit, produces QR codes and `syscity://pair/{code}` URIs, and supports base64url setup URLs
- Secret resolution cache — `SecretResolver` keeps per-provider payload caches (env, file, exec) and a per-reference result cache with TTL, plus `refresh()` and `refresh_reference()` for manual invalidation
- Audit logging for auth events — `AuditEventType` includes `Login`, `Logout`, and `TokenValidation`; `AuthManager` emits events via an attached `AuditLogger` on `create_session`, `validate_session`, and `revoke_session`
- Trusted proxy authentication — IP whitelist/CIDR, required headers, user extraction, allowUsers whitelist, audit logging
- Credential precedence — `env_first` vs `config_first` for token/password sources. `CredentialPrecedence` enum applied in `daemon.rs:apply_env_security_overrides()` and `apply_env_provider_overrides()`
- Secret scanning in tool outputs — `ContentFilter` with `SecretScanner` and `PiiDetector` wired into `ToolRegistry` during `Gateway::new()`
- JWT session management with scope-based access control
- User registration and lookup with admin flag support

