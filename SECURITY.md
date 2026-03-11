# Manta Security Documentation

This document outlines the security model, known vulnerabilities, and security practices for Manta.

## Security Model

### Defense in Depth

Manta implements multiple layers of security:

1. **Input Validation**: All user inputs are validated through JSON schema
2. **Sandboxing**: Shell commands run with restricted permissions
3. **Allowlists**: Explicit allowlists for paths and commands
4. **Rate Limiting**: Per-user request throttling
5. **Authentication**: Pairing codes for new users

## Known Security Issues

### Vulnerabilities (from cargo audit)

#### 1. RSA Timing Sidechannel (RUSTSEC-2023-0071)
- **Crate**: `rsa` v0.9.10
- **Severity**: Medium (5.9)
- **Issue**: Potential key recovery through timing sidechannels
- **Status**: No fixed upgrade available (upstream dependency via sqlx-mysql)
- **Impact**: Manta uses SQLite, not MySQL, so this vulnerability is **not exploitable** in Manta deployments
- **Mitigation**: We don't use RSA for cryptographic operations in Manta

#### 2. SQLx Binary Protocol Issue (RUSTSEC-2024-0363)
- **Crate**: `sqlx` v0.7.4
- **Severity**: High
- **Issue**: Binary Protocol Misinterpretation caused by Truncating or Overflowing Casts
- **Status**: Upgrade to >=0.8.1 required
- **Impact**: Affects SQLite protocol handling
- **Mitigation**: We recommend:
  - Regular database backups
  - Input validation on all database queries
  - Monitoring for unusual database behavior

### Unmaintained Dependencies

The following dependencies are unmaintained but don't have known security vulnerabilities:

1. **paste** (RUSTSEC-2024-0436) - Used by sqlx-core
2. **proc-macro-error** (RUSTSEC-2024-0370) - Used by teloxide
3. **rustls-pemfile** v1.0.4 (RUSTSEC-2025-0134) - Used by reqwest

These are transitive dependencies and will be updated when upstream crates release updates.

## Security Features

### Tool Security

#### Path Traversal Protection

Manta's `SecurityValidator` detects and blocks path traversal attempts:

```rust
// Blocked patterns:
- "../"              - Directory traversal
- "..\\"             - Windows traversal
- "~/.."             - Home directory escape
- "/.."              - Root escape
- "%2e%2e%2f"        - URL-encoded traversal
- "%252e%252e%252f"  - Double URL-encoded
- "//"               - Double slash
```

#### Command Injection Protection

The following characters are blocked in shell commands:
- `;` - Command separator
- `&` - Background process
- `|` - Pipe
- `$` - Variable substitution
- `` ` `` - Command substitution
- `$(` - Command substitution
- `${` - Variable expansion

### Configuration Security

#### Environment Variables

Sensitive configuration should use environment variables:

```yaml
provider:
  api_key: "${OPENAI_API_KEY}"  # Never hardcode secrets
```

#### Sandbox Configuration

Default sandbox settings:

```yaml
security:
  sandbox:
    enabled: true
    allowed_commands: ["ls", "cat", "grep", "curl"]
    forbidden_paths: ["/etc/passwd", "~/.ssh/*"]
    timeout_seconds: 30
```

### Network Security

#### TLS Configuration

- All HTTP clients use `rustls-tls` (native Rust TLS, not OpenSSL)
- Certificate validation is enabled by default
- No option to disable TLS verification in production

#### Domain Restrictions

Web tools support domain allowlisting/blocklisting:

```yaml
tools:
  web:
    blocked_domains: ["localhost", "127.0.0.1", "10.*", "192.168.*"]
```

## Security Best Practices

### Deployment

1. **Run as non-root user**
   ```bash
   useradd -r -s /bin/false manta
   ```

2. **Use read-only filesystem where possible**
   ```docker
   --read-only --tmpfs /tmp
   ```

3. **Limit network access**
   - Only expose necessary ports
   - Use internal networks for database connections

4. **Enable rate limiting**
   ```yaml
   security:
     rate_limits:
       requests_per_minute: 30
   ```

### Secret Management

1. Never commit secrets to version control
2. Use environment variables or secret management systems
3. Rotate API keys regularly
4. Use different keys for different environments

### Monitoring

Monitor for:
- Unusual API request patterns
- Path traversal attempts in logs
- Rate limit violations
- Failed authentication attempts

## Security Checklist

Before deploying Manta:

- [ ] Changed default API keys
- [ ] Configured allowlists appropriately
- [ ] Enabled sandbox mode
- [ ] Set up rate limiting
- [ ] Configured log rotation
- [ ] Running as non-root user
- [ ] Firewall rules configured
- [ ] Regular backups scheduled
- [ ] Monitoring alerts configured

## Reporting Security Issues

If you discover a security vulnerability:

1. **DO NOT** open a public issue
2. Email security concerns to: security@example.com
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Possible impact
   - Suggested fix (if any)

We will respond within 48 hours and work on a fix.

## Security Update Policy

- Critical vulnerabilities: Fix within 7 days
- High severity: Fix within 30 days
- Medium/Low severity: Fix in next scheduled release
- Dependency updates: Monthly review

## Audit History

| Date | Auditor | Scope | Results |
|------|---------|-------|---------|
| 2024-03 | cargo-audit | Dependencies | 2 vulnerabilities, 3 unmaintained |

## References

- [RustSec Advisory Database](https://rustsec.org/)
- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
- [CIS Docker Benchmark](https://www.cisecurity.org/benchmark/docker)
