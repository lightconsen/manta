//! Persistent connector state machine (`states.json`).
//!
//! Tracks each installed connector across gateway restarts, mirroring the
//! reference implementation's versioned `connector-states` file:
//!
//! ```text
//! Installed → Enabled ⇄ Disabled
//!      └──────────┴─────────→ Error(reason)
//! ```
//!
//! Uninstalling an `Enabled` connector is refused — the caller must `disable`
//! first so the MCP connection is torn down before its package disappears.
//!
//! The file carries a `version` field; [`StateStore::load`] runs the recorded
//! version through a step-wise migration table up to
//! [`STATE_VERSION`](STATE_VERSION) so future schema changes stay compatible
//! with on-disk files written by older builds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Current on-disk schema version.
pub const STATE_VERSION: u32 = 1;

// ─────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────

/// Lifecycle state of one installed connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// Package present but never enabled (or explicitly disabled after
    /// install without ever being enabled).
    Installed,
    /// Active: an MCP-backed connector is connected via `McpManager`.
    Enabled,
    /// Present but deactivated; no connection is held.
    Disabled,
    /// Last enable/connect attempt failed; `error` carries the reason.
    Error,
}

/// One connector's persisted record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorRecord {
    /// Connector id (key duplicated inside for self-contained entries).
    pub id: String,
    /// Installed package version (cache directory name).
    pub version: String,
    /// Current lifecycle state.
    pub state: StateKind,
    /// Failure reason when `state == Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Skill names installed into the user skills dir on this connector's
    /// behalf — removed again on uninstall.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// When the connector was first installed.
    pub installed_at: DateTime<Utc>,
    /// Last state transition.
    pub updated_at: DateTime<Utc>,
}

/// Root document of `states.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorStateFile {
    /// On-disk schema version, migrated up to [`STATE_VERSION`] on load.
    /// Defaults to 0 so pre-versioning files migrate forward.
    #[serde(default)]
    pub version: u32,
    /// Per-connector records keyed by id.
    #[serde(default)]
    pub connectors: HashMap<String, ConnectorRecord>,
}

impl Default for ConnectorStateFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            connectors: HashMap::new(),
        }
    }
}

impl ConnectorStateFile {
    /// Step-wise migrations from any historical version to [`STATE_VERSION`].
    ///
    /// Each arm upgrades exactly one step; the loop re-reads the file version
    /// until it reaches [`STATE_VERSION`] or hits an unknown (too new) format.
    fn migrate(&mut self) -> crate::Result<()> {
        loop {
            match self.version {
                STATE_VERSION => return Ok(()),
                // v0 was never shipped (pre-release layouts wrote no version
                // field); treat it as v1 content missing the marker.
                0 => {
                    self.version = 1;
                }
                other => {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "connector states.json version {other} is newer than this build \
                         supports ({}); upgrade Syscity first",
                        STATE_VERSION
                    )));
                }
            }
        }
    }
}

// ─────────────────────────────────────────────
// Store
// ─────────────────────────────────────────────

/// Filesystem-backed state store for `<connectors_dir>/states.json`.
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    /// Store rooted at `dir/states.json`.
    pub fn new(dir: &Path) -> Self {
        Self { path: dir.join("states.json") }
    }

    /// The underlying file path (exposed for diagnostics and tests).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load and migrate the state file.
    ///
    /// Missing file → empty default. Corrupt file → the damaged file is moved
    /// aside as `.bak` and an empty store is returned (a broken states file
    /// must not take the whole connector subsystem down).
    pub async fn load(&self) -> crate::Result<ConnectorStateFile> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConnectorStateFile::default());
            }
            Err(e) => {
                return Err(crate::error::SyscityError::IoContext {
                    context: format!("Failed to read {}", self.path.display()),
                    source: e,
                });
            }
        };

        let mut file: ConnectorStateFile = match serde_json::from_str(&raw) {
            Ok(file) => file,
            Err(e) => {
                warn!(
                    "Connector states file {} is corrupt ({e}); archiving as .bak and starting fresh",
                    self.path.display()
                );
                let bak = self.path.with_extension("json.bak");
                let _ = tokio::fs::rename(&self.path, &bak).await;
                return Ok(ConnectorStateFile::default());
            }
        };
        let from_version = file.version;
        file.migrate()?;
        if from_version != STATE_VERSION {
            // Migration moved the document forward — persist the upgraded form.
            self.save(&file).await?;
            info!("Migrated connector states v{from_version} → v{STATE_VERSION}");
        }
        Ok(file)
    }

    /// Atomically persist the state file (write temp + rename).
    pub async fn save(&self, file: &ConnectorStateFile) -> crate::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(file).map_err(|e| {
            crate::error::SyscityError::Internal(format!("Serialize connector states: {e}"))
        })?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json)
            .await
            .map_err(|e| crate::error::SyscityError::IoContext {
                context: format!("Failed to write {}", tmp.display()),
                source: e,
            })?;
        tokio::fs::rename(&tmp, &self.path).await.map_err(|e| {
            crate::error::SyscityError::IoContext {
                context: format!("Failed to replace {}", self.path.display()),
                source: e,
            }
        })
    }

    /// Apply a mutation to the state file under load-modify-save.
    pub async fn update(
        &self,
        f: impl FnOnce(&mut ConnectorStateFile) -> crate::Result<()>,
    ) -> crate::Result<()> {
        let mut file = self.load().await?;
        f(&mut file)?;
        self.save(&file).await
    }
}

/// Insert or replace a record with a fresh `updated_at`.
pub(crate) fn put_record(file: &mut ConnectorStateFile, record: ConnectorRecord) {
    file.connectors.insert(record.id.clone(), record);
}

/// Build an initial record for a freshly installed connector.
pub(crate) fn new_record(id: &str, version: &str) -> ConnectorRecord {
    let now = Utc::now();
    ConnectorRecord {
        id: id.to_string(),
        version: version.to_string(),
        state: StateKind::Installed,
        error: None,
        skills: Vec::new(),
        installed_at: now,
        updated_at: now,
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("syscity_connector_state_{tag}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn roundtrip_records() {
        let dir = temp_dir("roundtrip");
        let store = StateStore::new(&dir);

        let mut rec = new_record("linear-mcp", "1.0.0");
        rec.state = StateKind::Enabled;
        store
            .update(|f| {
                put_record(f, rec.clone());
                Ok(())
            })
            .await
            .unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.version, STATE_VERSION);
        let got = loaded.connectors.get("linear-mcp").unwrap();
        assert_eq!(got.state, StateKind::Enabled);
        assert_eq!(got.version, "1.0.0");
        assert_eq!(got.installed_at, rec.installed_at);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn migrates_v0_unmarked_file_to_current() {
        let dir = temp_dir("migrate");
        // A pre-release layout with records but no version marker.
        std::fs::write(
            dir.join("states.json"),
            r#"{ "connectors": { "legacy": { "id": "legacy", "version": "0.9.0",
                 "state": "installed", "installed_at": "2026-01-01T00:00:00Z",
                 "updated_at": "2026-01-01T00:00:00Z" } } }"#,
        )
        .unwrap();

        let loaded = StateStore::new(&dir).load().await.unwrap();
        assert_eq!(loaded.version, STATE_VERSION);
        assert!(loaded.connectors.contains_key("legacy"));
        // The migrated form is persisted back to disk.
        let raw = std::fs::read_to_string(dir.join("states.json")).unwrap();
        assert!(raw.contains("\"version\": 1"), "{raw}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_future_versions_instead_of_dropping_data() {
        let dir = temp_dir("future");
        std::fs::write(dir.join("states.json"), r#"{ "version": 99 }"#).unwrap();
        let err = StateStore::new(&dir).load().await.unwrap_err();
        assert!(err.to_string().contains("newer than this build"), "{err}");
        // The original file must be untouched — data loss is worse than a
        // hard failure here.
        assert!(dir.join("states.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn corrupt_file_backed_up_and_reset() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("states.json"), "{ not json !!!").unwrap();

        let loaded = StateStore::new(&dir).load().await.unwrap();
        assert!(loaded.connectors.is_empty());
        assert!(dir.join("states.json.bak").exists(), "damaged file archived");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn missing_file_is_default() {
        let dir = temp_dir("missing");
        let loaded = StateStore::new(&dir).load().await.unwrap();
        assert_eq!(loaded.version, STATE_VERSION);
        assert!(loaded.connectors.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
