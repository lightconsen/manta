//! Declarative lifecycle executor for CLI-backed connectors.
//!
//! Executes the per-platform commands declared in a connector manifest's
//! `lifecycle` section (see [`LifecycleSpec`](super::manifest::LifecycleSpec))
//! — dependency install, version verification, and auth login/status/logout.
//!
//! ## Security model
//!
//! These commands come from third-party packages, so they are gated by
//! construction:
//!
//! - They only ever run on an **explicit user/agent action** (`connector_install`,
//!   `connector_auth`, …). Nothing here runs automatically at startup.
//! - `run_init` runs once per install; auth/login is interactive and may block
//!   on user input in the spawned process.
//! - The agent-facing path goes through `McpConnectionTool`, which already
//!   carries approval requirements for system-touching actions.
//!
//! Commands run through the platform shell (`sh -c` / `cmd /C`) because they
//! are declared as shell lines, with a hard timeout and truncated captured
//! output.

use std::time::Duration;

use tracing::{info, warn};

use crate::error::SyscityError;

use super::manifest::{
    resolve_command, ConnectorManifest, LifecycleSpec, Platform, PlatformCommands,
};

/// Result of one lifecycle command execution.
#[derive(Debug, Clone)]
pub struct LifecycleOutput {
    /// Exit code (process-dependent semantics).
    pub code: i32,
    /// Combined stdout (truncated).
    pub stdout: String,
    /// Combined stderr (truncated).
    pub stderr: String,
}

impl LifecycleOutput {
    fn success(&self) -> bool {
        self.code == 0
    }
}

const OUTPUT_TRUNCATE: usize = 4000;
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Runs lifecycle commands declared by connector manifests.
pub struct LifecycleRunner {
    timeout: Duration,
    dry_run: bool,
}

impl Default for LifecycleRunner {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT_SECS)
    }
}

impl LifecycleRunner {
    /// Runner with a per-command timeout.
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs.max(1)),
            dry_run: false,
        }
    }

    /// Resolve commands but never spawn processes (test seam).
    pub fn dry_run() -> Self {
        Self {
            timeout: Duration::from_secs(1),
            dry_run: true,
        }
    }

    // ── Manifest-level entry points ─────────────────────────────────────

    /// Run the manifest's `init` hook (dependency installation).
    ///
    /// A manifest without an `init` section succeeds trivially so callers can
    /// invoke this unconditionally after unpacking a package.
    pub async fn run_init(
        &self,
        manifest: &ConnectorManifest,
    ) -> crate::Result<Option<LifecycleOutput>> {
        let Some(spec) = &manifest.lifecycle else {
            return Ok(None);
        };
        let Some(commands) = &spec.init else {
            return Ok(None);
        };
        let cmd = require_command(commands, "init")?;
        info!("connector {}: init> {cmd}", manifest.connector.id);
        if self.dry_run {
            return Ok(Some(LifecycleOutput {
                code: 0,
                stdout: format!("dry-run: {cmd}"),
                stderr: String::new(),
            }));
        }
        let out = self.execute(&cmd).await?;
        out.ensure("init").map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "connector {} init failed: {e}",
                manifest.connector.id
            ))
        })?;
        Ok(Some(out))
    }

    /// Run the manifest's `version_check` and verify `min_version`.
    ///
    /// Missing hooks or missing `min_version` count as satisfied — the check
    /// is advisory when the publisher did not pin anything.
    pub async fn run_version_check(
        &self,
        manifest: &ConnectorManifest,
    ) -> crate::Result<VersionCheckOutcome> {
        let Some(spec) = &manifest.lifecycle else {
            return Ok(VersionCheckOutcome::NotDeclared);
        };
        let Some(check) = &spec.version_check else {
            return Ok(VersionCheckOutcome::NotDeclared);
        };
        let Some(min_version) = &check.min_version else {
            return Ok(VersionCheckOutcome::NotDeclared);
        };
        let cmd = require_command(&check.command, "version_check")?;

        let output = if self.dry_run {
            LifecycleOutput {
                code: 0,
                stdout: format!("dry-run: {cmd}"),
                stderr: String::new(),
            }
        } else {
            self.execute(&cmd).await?
        };

        // First whitespace-separated token of stdout should be the version;
        // tolerate a leading "v" (common in tool output).
        let reported = output
            .stdout
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_start_matches(['v', 'V'])
            .to_string();
        let parsed = crate::skills::semver::Version::parse(&reported);
        let req = crate::skills::semver::VersionReq::parse(min_version);

        match (parsed, req) {
            (Ok(ref v), Ok(ref r)) if r.matches(v) => Ok(VersionCheckOutcome::Satisfied),
            (Ok(v), Ok(_)) => Err(crate::error::SyscityError::Validation(format!(
                "connector {} requires runtime version {min_version} (got {v}); \
                 run its init/auth setup first",
                manifest.connector.id
            ))),
            (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                warn!("connector {} version_check misconfigured: {e}", manifest.connector.id);
                Ok(VersionCheckOutcome::Unverifiable)
            }
            (Err(a), Err(b)) => Err(crate::error::SyscityError::Validation(format!(
                "connector {} version_check failed to parse: reported={reported:?} ({a}), \
                 requirement={min_version:?} ({b})",
                manifest.connector.id
            ))),
        }
    }

    /// Auth login (interactive; long timeout applies).
    pub async fn run_auth_login(
        &self,
        manifest: &ConnectorManifest,
    ) -> crate::Result<LifecycleOutput> {
        self.run_auth(manifest, "login", false).await
    }

    /// Auth status probe (read-only).
    pub async fn run_auth_status(
        &self,
        manifest: &ConnectorManifest,
    ) -> crate::Result<Option<String>> {
        let line = self.auth_command(manifest, "status")?;
        match line {
            None => Ok(None),
            Some(cmd) if self.dry_run => Ok(Some(format!("dry-run: {cmd}"))),
            Some(cmd) => {
                let out = self.execute(&cmd).await?;
                if out.success() {
                    Ok(Some(first_line(&out.stdout)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Auth logout / credential reset.
    pub async fn run_auth_logout(&self, manifest: &ConnectorManifest) -> crate::Result<()> {
        if self.auth_command(manifest, "logout")?.is_none() {
            return Ok(());
        }
        self.run_auth(manifest, "logout", true).await.map(|_| ())
    }

    // ── Internals ───────────────────────────────────────────────────────

    fn auth_spec<'a>(
        manifest: &'a ConnectorManifest,
        which: &str,
    ) -> crate::Result<&'a PlatformCommands> {
        let spec: &LifecycleSpec =
            manifest
                .lifecycle
                .as_ref()
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!(
                        "connector {} has no lifecycle section",
                        manifest.connector.id
                    ),
                })?;
        let auth = spec
            .auth
            .as_ref()
            .ok_or_else(|| crate::error::SyscityError::NotFound {
                resource: format!("connector {} has no auth section", manifest.connector.id),
            })?;
        match which {
            "login" => auth.login.as_ref(),
            "status" => auth.status.as_ref(),
            "logout" => auth.logout.as_ref(),
            _ => unreachable!("unknown auth verb"),
        }
        .ok_or_else(|| crate::error::SyscityError::NotFound {
            resource: format!("connector {} has no auth.{which} command", manifest.connector.id),
        })
    }

    async fn run_auth(
        &self,
        manifest: &ConnectorManifest,
        which: &str,
        optional_ok: bool,
    ) -> crate::Result<LifecycleOutput> {
        let cmd = match Self::auth_spec(manifest, which) {
            Ok(map) => require_command(map, which)?,
            Err(e) => {
                if optional_ok && matches!(e, crate::error::SyscityError::NotFound { .. }) {
                    return Ok(LifecycleOutput {
                        code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                return Err(e);
            }
        };
        info!("connector {}: auth.{which}> {cmd}", manifest.connector.id);
        if self.dry_run {
            return Ok(LifecycleOutput {
                code: 0,
                stdout: format!("dry-run: {cmd}"),
                stderr: String::new(),
            });
        }
        let out = self.execute(&cmd).await?;
        out.ensure(which).map_err(SyscityError::Internal)?;
        Ok(out)
    }

    fn auth_command(
        &self,
        manifest: &ConnectorManifest,
        which: &str,
    ) -> crate::Result<Option<String>> {
        match Self::auth_spec(manifest, which) {
            Ok(map) => Ok(resolve_command(map, Platform::native()).map(str::to_string)),
            Err(crate::error::SyscityError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Spawn one shell line under the platform shell with timeout + capture.
    async fn execute(&self, command: &str) -> crate::Result<LifecycleOutput> {
        #[cfg(unix)]
        let child = {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(command);
            c.stdout(std::process::Stdio::piped());
            c.stderr(std::process::Stdio::piped());
            c.spawn().map_err(|e| {
                crate::error::SyscityError::Internal(format!("Failed to spawn `{command}`: {e}"))
            })?
        };
        #[cfg(windows)]
        let child = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c.stdout(std::process::Stdio::piped());
            c.stderr(std::process::Stdio::piped());
            c.spawn().map_err(|e| {
                crate::error::SyscityError::Internal(format!("Failed to spawn `{command}`: {e}"))
            })?
        };

        let waited = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal(format!(
                    "Command `{command}` timed out after {:?}",
                    self.timeout
                ))
            })?
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Command `{command}` failed: {e}"))
            })?;

        Ok(LifecycleOutput {
            code: waited.status.code().unwrap_or(-1),
            stdout: truncate(String::from_utf8_lossy(&waited.stdout).into_owned()),
            stderr: truncate(String::from_utf8_lossy(&waited.stderr).into_owned()),
        })
    }
}

/// Whether a version check was satisfiable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionCheckOutcome {
    /// Declared and verified against min_version.
    Satisfied,
    /// No version_check / no min_version — nothing to enforce.
    NotDeclared,
    /// Declared but unparseable output/config; treated as non-fatal warning.
    Unverifiable,
}

fn require_command(map: &PlatformCommands, hook: &str) -> crate::Result<String> {
    resolve_command(map, Platform::native())
        .map(str::to_string)
        .ok_or_else(|| {
            crate::error::SyscityError::Validation(format!(
                "no `{hook}` command declared for platform {:?}",
                Platform::native().key()
            ))
        })
}

impl LifecycleOutput {
    fn ensure(&self, hook: &str) -> Result<(), String> {
        if self.success() {
            Ok(())
        } else {
            Err(format!(
                "`{hook}` exited with code {}; stderr: {}",
                self.code,
                first_line(&self.stderr)
            ))
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

fn truncate(s: String) -> String {
    if s.len() <= OUTPUT_TRUNCATE {
        return s;
    }
    let mut cut = OUTPUT_TRUNCATE;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…[truncated]", &s[..cut])
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::connectors::manifest::ConnectorManifest;

    fn cli_manifest() -> ConnectorManifest {
        ConnectorManifest::parse(
            r#"{
            "connector": { "id": "cli-tool", "display_name": "CLI Tool", "version": "1.0.0" },
            "lifecycle": {
                "runtime": { "type": "node", "version": ">=16" },
                "init": { "darwin": "echo installed", "linux": "echo installed",
                          "win32": "echo installed" },
                "version_check": { "command": {"darwin": "echo v1.2.3",
                                               "linux": "echo v1.2.3",
                                               "win32": "echo v1.2.3"},
                                   "min_version": ">=1.0.0" },
                "auth": {
                    "login":  { "darwin": "echo logging-in", "linux": "echo logging-in",
                                "win32": "echo logging-in" },
                    "status": { "darwin": "echo logged-in", "linux": "echo logged-in",
                                "win32": "echo logged-in" },
                    "logout": { "darwin": "echo logged-out", "linux": "echo logged-out",
                                "win32": "echo logged-out" }
                }
            }
        }"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn dry_run_resolves_commands_without_spawning() {
        let runner = LifecycleRunner::dry_run();
        let m = cli_manifest();

        let init = runner.run_init(&m).await.unwrap().unwrap();
        assert!(init.stdout.starts_with("dry-run: echo installed"), "{init:?}");

        // Dry-run cannot produce a parseable version (nothing executed), so
        // the check reports Unverifiable rather than Satisfied.
        assert_eq!(runner.run_version_check(&m).await.unwrap(), VersionCheckOutcome::Unverifiable);

        let login = runner.run_auth_login(&m).await.unwrap();
        assert!(login.stdout.contains("logging-in"), "{login:?}");

        let status = runner.run_auth_status(&m).await.unwrap().unwrap();
        assert!(status.contains("logged-in"), "{status}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_shell_execution_captures_output_and_codes() {
        let runner = LifecycleRunner::new(30);
        let out = runner.execute("echo hello-syscity").await.unwrap();
        assert_eq!(out.code, 0);
        assert_eq!(first_line(&out.stdout), "hello-syscity");

        let failed = runner.execute("exit 7").await.unwrap();
        assert_eq!(failed.code, 7);
        assert!(failed.ensure("probe").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_stuck_commands() {
        let runner = LifecycleRunner::new(1);
        let err = runner.execute("sleep 30").await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_check_rejects_old_versions() {
        let runner = LifecycleRunner::new(30);
        let mut m = cli_manifest();
        if let Some(lc) = &mut m.lifecycle {
            if let Some(vc) = &mut lc.version_check {
                vc.min_version = Some(">=99.0.0".to_string());
            }
        }
        let err = runner.run_version_check(&m).await.unwrap_err();
        assert!(err.to_string().contains("requires runtime version"), "{err}");
    }

    #[tokio::test]
    async fn version_check_not_declared_is_satisfied_by_default() {
        let runner = LifecycleRunner::dry_run();
        let m = ConnectorManifest::parse(
            r#"{
            "connector": { "id": "plain", "version": "" },
            "skills": ["skills"]
        }"#,
        )
        .unwrap();
        assert_eq!(runner.run_version_check(&m).await.unwrap(), VersionCheckOutcome::NotDeclared);
        assert!(runner.run_init(&m).await.unwrap().is_none());
    }

    /// Regression guard: the lifecycle test fixture must declare commands for
    /// every platform. CI runs on Linux, so darwin-only fixtures panic there
    /// ("no version_check command declared for platform linux").
    #[test]
    fn cli_manifest_resolves_commands_on_all_platforms() {
        let m = cli_manifest();
        let lc = m.lifecycle.as_ref().unwrap();
        for platform in [Platform::Darwin, Platform::Linux, Platform::Windows] {
            let vc = lc.version_check.as_ref().unwrap();
            assert!(
                resolve_command(&vc.command, platform).is_some(),
                "version_check command missing for {platform:?}"
            );
            let auth = lc.auth.as_ref().unwrap();
            for (name, map) in [
                ("login", auth.login.as_ref()),
                ("status", auth.status.as_ref()),
                ("logout", auth.logout.as_ref()),
            ] {
                assert!(
                    map.is_some_and(|m| resolve_command(m, platform).is_some()),
                    "auth.{name} command missing for {platform:?}"
                );
            }
        }
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let long = "🦑".repeat(OUTPUT_TRUNCATE); // multi-byte, forces boundary walk
        let got = truncate(long.clone());
        assert!(got.ends_with("[truncated]"));
        assert!(got.len() < long.len() + 20);
    }

    #[tokio::test]
    async fn missing_lifecycle_sections_error_cleanly() {
        let runner = LifecycleRunner::dry_run();
        let m = ConnectorManifest::parse(
            r#"{ "connector": { "id": "bare", "version": "" }, "mcp": null, "skills": ["s"] }"#,
        )
        .unwrap();
        let err = runner.run_auth_login(&m).await.unwrap_err();
        assert!(matches!(err, crate::error::SyscityError::NotFound { .. }));
    }
}
