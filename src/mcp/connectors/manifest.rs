//! Connector package manifest (`connector.json`) — the declarative DSL that
//! describes what a connector provides and how its host-managed lifecycle
//! works.
//!
//! A connector package is a directory containing a `connector.json` at its
//! root, mirroring the WorkBuddy connector entry shapes:
//!
//! - MCP-backed (the `linear-mcp` shape): an `mcp` section pointing at an
//!   HTTP/SSE endpoint or stdio command; enabling the connector connects it
//!   through [`McpManager`](super::super::McpManager).
//! - CLI-backed (the `dingtalk` shape): a `lifecycle` section declaring
//!   per-platform setup / version-check / auth commands plus bundled skills
//!   that teach the agent how to drive the installed CLI.
//!
//! At least one of `mcp`, `lifecycle`, or `skills` must be present.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::super::{McpServerConfig, McpTransport};

// ─────────────────────────────────────────────
// Manifest types
// ─────────────────────────────────────────────

/// Root document of a `connector.json` package manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    /// Identity and display metadata.
    pub connector: ConnectorMeta,
    /// Optional MCP server this connector exposes when enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSection>,
    /// Optional host-managed lifecycle hooks (setup / version check / auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<LifecycleSpec>,
    /// Optional directories (relative to the package root) holding bundled
    /// skills, each a `SKILL.md` folder installed at connector install time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
}

/// Identity block of a connector manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorMeta {
    /// Stable identifier (`[a-z0-9-]+`). Also the server id used in
    /// `McpManager` when the connector has an `mcp` section.
    pub id: String,
    /// Human-readable name shown in listings.
    #[serde(default)]
    pub display_name: String,
    /// One-line description of what the connector provides.
    #[serde(default)]
    pub description: String,
    /// Package version (semver string, informational here — the authoritative
    /// version is the cache directory name).
    #[serde(default)]
    pub version: String,
    /// Icon path relative to the package root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The `mcp` manifest section — a subset of [`McpServerConfig`] fields that a
/// third-party package may declare.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpSection {
    /// Transport to use (default: stdio).
    #[serde(default)]
    pub transport: McpTransport,
    /// URL for SSE / streamable-HTTP transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Command for the stdio transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments for the stdio command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Static environment variables (supports `$VAR` expansion at connect).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Connect automatically when the connector is enabled (default true).
    #[serde(default = "default_true")]
    pub auto_connect: bool,
}

fn default_true() -> bool {
    true
}

impl McpSection {
    /// Convert into a full [`McpServerConfig`] for `McpManager`.
    ///
    /// Fields not expressible in a connector package (OAuth endpoints, tuning)
    /// fall back to their defaults; they can still be tuned by hand through
    /// `config.toml [mcp.servers.*]` under the same id.
    pub fn to_server_config(&self) -> McpServerConfig {
        McpServerConfig {
            transport: self.transport.clone(),
            url: self.url.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            env: self.env.clone(),
            auto_connect: self.auto_connect,
            ..Default::default()
        }
    }
}

/// Target platform key used by lifecycle command maps.
///
/// Uses the same keys as the reference implementation (`darwin`, `linux`,
/// `win32`) so published connector packages stay portable across both hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Darwin,
    Linux,
    Windows,
}

impl Platform {
    /// The platform this build runs on.
    pub fn native() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// The manifest key for this platform.
    pub fn key(self) -> &'static str {
        match self {
            Platform::Darwin => "darwin",
            Platform::Linux => "linux",
            Platform::Windows => "win32",
        }
    }
}

/// Per-platform command map (`{"darwin": "...", "linux": "...", "win32": "..."}`).
pub type PlatformCommands = HashMap<String, String>;

/// Runtime requirement declared by a CLI-backed connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    /// Runtime kind (e.g. "node"). Informational today; enforced via the
    /// version check below.
    #[serde(rename = "type")]
    pub kind: String,
    /// Version requirement expression understood by
    /// [`VersionReq`](crate::skills::semver::VersionReq), e.g. `">=16"`.
    #[serde(default)]
    pub version: Option<String>,
}

/// The `version_check` lifecycle block: a command whose output should parse as
/// a version satisfying `min_version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckSpec {
    /// Per-platform version-printing command.
    pub command: PlatformCommands,
    /// Minimum acceptable version (semver requirement expression).
    #[serde(default)]
    pub min_version: Option<String>,
}

/// Auth lifecycle commands for CLI-backed connectors.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthSpec {
    /// Interactive login (user-driven, run in the foreground).
    #[serde(default)]
    pub login: Option<PlatformCommands>,
    /// Non-destructive status probe (safe to surface anywhere).
    #[serde(default)]
    pub status: Option<PlatformCommands>,
    /// Logout / credential reset.
    #[serde(default)]
    pub logout: Option<PlatformCommands>,
}

/// Host-managed lifecycle declaration for CLI-backed connectors.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LifecycleSpec {
    /// Declared runtime dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeSpec>,
    /// Dependency installation commands (run once during install).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init: Option<PlatformCommands>,
    /// Post-install version verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_check: Option<VersionCheckSpec>,
    /// Authentication lifecycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
}

// ─────────────────────────────────────────────
// Resolution & validation
// ─────────────────────────────────────────────

/// Pick the command for `platform` out of a per-platform map.
pub fn resolve_command(map: &PlatformCommands, platform: Platform) -> Option<&str> {
    map.get(platform.key()).map(String::as_str)
}

impl ConnectorManifest {
    /// Parse and validate a manifest from raw JSON bytes.
    pub fn parse(json: &str) -> crate::Result<Self> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|e| crate::error::SyscityError::Validation(format!("connector.json: {e}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Load a manifest from a package directory's `connector.json`.
    pub async fn load(package_dir: &std::path::Path) -> crate::Result<Self> {
        let path = package_dir.join("connector.json");
        let json = tokio::fs::read_to_string(&path).await.map_err(|e| {
            crate::error::SyscityError::IoContext {
                context: format!("Failed to read {}", path.display()),
                source: e,
            }
        })?;
        Self::parse(&json)
    }

    /// Structural validation applied on every parse.
    pub fn validate(&self) -> crate::Result<()> {
        let meta = &self.connector;
        if meta.id.is_empty()
            || !meta
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(crate::error::SyscityError::Validation(format!(
                "connector id {:?} must be non-empty [a-z0-9-]",
                meta.id
            )));
        }
        if !meta.version.is_empty() && crate::skills::semver::Version::parse(&meta.version).is_err()
        {
            return Err(crate::error::SyscityError::Validation(format!(
                "connector {} has invalid semver version {:?}",
                meta.id, meta.version
            )));
        }
        if self.mcp.is_none() && self.lifecycle.is_none() && self.skills.is_empty() {
            return Err(crate::error::SyscityError::Validation(format!(
                "connector {} declares nothing to provide \
                 (need at least one of mcp / lifecycle / skills)",
                meta.id
            )));
        }
        // An mcp section without any usable endpoint is a authoring error —
        // fail loudly instead of producing a config that can never connect.
        if let Some(mcp) = &self.mcp {
            match mcp.transport {
                McpTransport::Stdio => {
                    if mcp.command.is_none() {
                        return Err(crate::error::SyscityError::Validation(format!(
                            "connector {}: stdio transport requires \"command\"",
                            meta.id
                        )));
                    }
                }
                McpTransport::Sse | McpTransport::StreamableHttp => {
                    if mcp.url.is_none() {
                        return Err(crate::error::SyscityError::Validation(format!(
                            "connector {}: {} transport requires \"url\"",
                            meta.id,
                            match mcp.transport {
                                McpTransport::Sse => "sse",
                                _ => "streamable_http",
                            }
                        )));
                    }
                }
                McpTransport::InProcess => {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "connector {}: the in-process transport is reserved for built-in servers",
                        meta.id
                    )));
                }
            }
        }
        Ok(())
    }

    /// True when enabling this connector should open an MCP connection.
    pub fn provides_mcp(&self) -> bool {
        self.mcp.is_some()
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const LINEAR_MCP_JSON: &str = r#"{
        "connector": { "id": "linear-mcp", "display_name": "Linear",
                       "description": "Linear issue tracking", "version": "1.0.0" },
        "mcp": { "transport": "streamable_http", "url": "https://mcp.linear.app/mcp" }
    }"#;

    const DINGTALK_CLI_JSON: &str = r#"{
        "connector": { "id": "dingtalk", "display_name": "DingTalk", "version": "0.2.0" },
        "lifecycle": {
            "runtime": { "type": "node", "version": ">=16" },
            "init": { "darwin": "npm install -g dingtalk-cli", "linux": "npm install -g dingtalk-cli" },
            "version_check": { "command": {"darwin": "dws --version"}, "min_version": "1.0.59" },
            "auth": {
                "login":  { "darwin": "dws auth login -y" },
                "status": { "darwin": "dws auth status" },
                "logout": { "darwin": "dws auth reset" }
            }
        },
        "skills": ["skills"]
    }"#;

    #[test]
    fn parses_mcp_backed_manifest() {
        let m = ConnectorManifest::parse(LINEAR_MCP_JSON).unwrap();
        assert_eq!(m.connector.id, "linear-mcp");
        assert!(m.provides_mcp());
        assert!(m.lifecycle.is_none());
        let cfg = m.mcp.unwrap().to_server_config();
        assert_eq!(cfg.transport, McpTransport::StreamableHttp);
        assert_eq!(cfg.url.as_deref(), Some("https://mcp.linear.app/mcp"));
        assert!(cfg.auto_connect);
    }

    #[test]
    fn parses_cli_backed_manifest() {
        let m = ConnectorManifest::parse(DINGTALK_CLI_JSON).unwrap();
        assert!(!m.provides_mcp());
        let lc = m.lifecycle.as_ref().unwrap();
        let cmd =
            resolve_command(lc.auth.as_ref().unwrap().login.as_ref().unwrap(), Platform::Darwin);
        assert_eq!(cmd, Some("dws auth login -y"));
        // linux has no auth.login entry — resolution must be None, not panic.
        assert_eq!(
            resolve_command(lc.auth.as_ref().unwrap().login.as_ref().unwrap(), Platform::Linux),
            None
        );
        assert_eq!(m.skills, vec!["skills"]);
    }

    #[test]
    fn rejects_invalid_id() {
        let bad = r#"{ "connector": { "id": "Bad_ID!", "version": "" }, "skills": [] }"#;
        let err = ConnectorManifest::parse(bad).unwrap_err();
        assert!(err.to_string().contains("[a-z0-9-]"), "{err}");
    }

    #[test]
    fn rejects_version_out_of_content() {
        let bad = r#"{
            "connector": { "id": "ok-id", "version": "not-semver" },
            "skills": ["s"]
        }"#;
        let err = ConnectorManifest::parse(bad).unwrap_err();
        assert!(err.to_string().contains("invalid semver"), "{err}");
    }

    #[test]
    fn rejects_connector_providing_nothing() {
        let bad = r#"{ "connector": { "id": "empty" } }"#;
        let err = ConnectorManifest::parse(bad).unwrap_err();
        assert!(err.to_string().contains("declares nothing"), "{err}");
    }

    #[test]
    fn rejects_stdio_without_command_and_http_without_url() {
        let bad_stdio = r#"{
            "connector": { "id": "a" },
            "mcp": { "transport": "stdio" }
        }"#;
        let err = ConnectorManifest::parse(bad_stdio).unwrap_err();
        assert!(err.to_string().contains("requires \"command\""), "{err}");

        let bad_http = r#"{
            "connector": { "id": "b" },
            "mcp": { "transport": "sse" }
        }"#;
        let err = ConnectorManifest::parse(bad_http).unwrap_err();
        assert!(err.to_string().contains("requires \"url\""), "{err}");
    }

    #[test]
    fn rejects_in_process_transport() {
        let bad = r#"{
            "connector": { "id": "c", "skills": ["s"] },
            "mcp": { "transport": "in_process" }
        }"#;
        let err = ConnectorManifest::parse(bad).unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");
    }

    #[test]
    fn platform_keys_match_reference_naming() {
        assert_eq!(Platform::Darwin.key(), "darwin");
        assert_eq!(Platform::Linux.key(), "linux");
        assert_eq!(Platform::Windows.key(), "win32");
        // Native must resolve to one of the three documented keys.
        assert!(matches!(Platform::native().key(), "darwin" | "linux" | "win32"));
    }
}
