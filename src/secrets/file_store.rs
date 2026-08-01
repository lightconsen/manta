//! Tier 2 backend — 0600/0700 atomic-write file storage.
//!
//! Each `(namespace, entity)` maps to one TOML file:
//!
//! ```text
//! ~/.syscity/secrets/{namespace}/{entity}.toml
//! ```
//!
//! The file structure is a `[secrets] kind = "value"` table. The directory is
//! `0700`, the file is `0600` (write a temp file → `set_permissions` → atomic
//! rename, so the final file never carries default permissive permissions).
//! The write pattern is absorbed from the retired `src/mcp/env_store.rs`.
//!
//! Legacy format `~/.syscity/mcp_env/{id}.toml` (`[env]` table) is read for
//! compatibility: on first startup `migrate_legacy_mcp_env()` moves it into the
//! new location and then deletes the old files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::SyscityError;
use crate::secrets::store::{SecretId, SecretOrigin, SecretStore};

/// Top-level secrets directory (`~/.syscity/secrets`).
pub fn secrets_root_dir() -> PathBuf {
    crate::dirs::syscity_dir().join("secrets")
}

/// Reject entity names that could escape the storage directory.
pub fn sanitize_entity(entity: &str) -> crate::Result<String> {
    if entity.is_empty()
        || entity == "."
        || entity == ".."
        || entity.contains('/')
        || entity.contains('\\')
        || entity.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(SyscityError::Validation(format!("invalid secret entity: {entity:?}")));
    }
    Ok(entity.to_string())
}

/// On-disk file content: `[secrets] kind = "value"`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretsFile {
    #[serde(default)]
    secrets: HashMap<String, String>,
}

/// Legacy `mcp_env` file content (`[env]` table) — only read during migration.
#[derive(Debug, Serialize, Deserialize)]
struct LegacyEnvFile {
    #[serde(default)]
    env: HashMap<String, String>,
}

/// Tier 2 file backend.
#[derive(Debug, Clone)]
pub struct FileStore {
    namespace: String,
    root: PathBuf,
}

impl FileStore {
    /// Create a file backend bound to a namespace.
    pub fn new(namespace: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            root: secrets_root_dir(),
        }
    }

    /// Test helper: point the root at a temp directory to keep tests hermetic.
    #[cfg(test)]
    pub(crate) fn with_root(namespace: &str, root: PathBuf) -> Self {
        Self {
            namespace: namespace.to_string(),
            root,
        }
    }

    /// Namespace directory: `{root}/{namespace}`.
    pub fn base_dir(&self) -> PathBuf {
        self.root.join(&self.namespace)
    }

    /// Entity file path (validates the entity name first).
    pub fn path_for(&self, entity: &str) -> crate::Result<PathBuf> {
        let entity = sanitize_entity(entity)?;
        Ok(self.base_dir().join(format!("{entity}.toml")))
    }

    /// Ensure the storage directory exists with correct permissions.
    pub async fn validate_store(&self) -> crate::Result<()> {
        let dir = self.base_dir();
        tokio::fs::create_dir_all(&dir).await?;
        set_dir_perms(&dir).await?;
        Ok(())
    }

    /// Read an entity's whole map; a missing file yields an empty map.
    pub async fn get_all(&self, entity: &str) -> crate::Result<HashMap<String, String>> {
        let path = self.path_for(entity)?;
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                let file: SecretsFile = toml::from_str(&content)?;
                Ok(file.secrets)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Overwrite-write an entity's whole map.
    pub async fn set_all(&self, entity: &str, map: &HashMap<String, String>) -> crate::Result<()> {
        let path = self.path_for(entity)?;
        write_atomically(&path, &SecretsFile { secrets: map.clone() }).await
    }

    /// Delete an entity's whole file (missing is not an error).
    pub async fn delete_entity(&self, entity: &str) -> crate::Result<()> {
        let path = self.path_for(entity)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether an entity already has a file.
    pub async fn has_entity(&self, entity: &str) -> bool {
        match self.path_for(entity) {
            Ok(path) => tokio::fs::metadata(path)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[async_trait::async_trait]
impl SecretStore for FileStore {
    async fn get(&self, id: &SecretId) -> crate::Result<Option<String>> {
        let map = self.get_all(&id.entity).await?;
        Ok(map.get(&id.kind).cloned())
    }

    async fn set(&self, id: &SecretId, value: &str, _origin: SecretOrigin) -> crate::Result<()> {
        let mut map = self.get_all(&id.entity).await?;
        map.insert(id.kind.clone(), value.to_string());
        self.set_all(&id.entity, &map).await
    }

    async fn delete(&self, id: &SecretId) -> crate::Result<()> {
        let mut map = self.get_all(&id.entity).await?;
        if map.remove(&id.kind).is_some() {
            if map.is_empty() {
                self.delete_entity(&id.entity).await?;
            } else {
                self.set_all(&id.entity, &map).await?;
            }
        }
        Ok(())
    }

    async fn has(&self, id: &SecretId) -> bool {
        matches!(self.get(id).await, Ok(Some(_)))
    }
}

// ─────────────────────────────────────────────
// Atomic write + permissions
// ─────────────────────────────────────────────

/// Write a temp file → tighten permissions → atomic rename.
async fn write_atomically(path: &Path, file: &SecretsFile) -> crate::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| SyscityError::Internal("secret store path has no parent".to_string()))?;
    tokio::fs::create_dir_all(dir).await?;
    set_dir_perms(dir).await?;

    let content = toml::to_string(file)?;
    let tmp = path.with_extension("toml.tmp");
    tokio::fs::write(&tmp, content).await?;
    set_file_perms(&tmp).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
async fn set_dir_perms(dir: &Path) -> crate::Result<()> {
    tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_dir_perms(_dir: &Path) -> crate::Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn set_file_perms(path: &Path) -> crate::Result<()> {
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_file_perms(_path: &Path) -> crate::Result<()> {
    Ok(())
}

// ─────────────────────────────────────────────
// Legacy format migration (~/.syscity/mcp_env → ~/.syscity/secrets/mcp-env)
// ─────────────────────────────────────────────

/// Migrate the old `~/.syscity/mcp_env/{id}.toml` (`[env]` table) once into
/// `~/.syscity/secrets/mcp-env/{id}.toml` (`[secrets]` table), then delete the
/// old directory.
///
/// The migration must be atomic and rollback-safe: each file is written to its
/// new location before the old file is removed; on any failure the current
/// state is kept and a warning is logged. The old directory is removed once
/// empty.
pub async fn migrate_legacy_mcp_env() -> crate::Result<()> {
    let store = FileStore::new("mcp-env");
    migrate_legacy_mcp_env_with_store(crate::dirs::syscity_dir().join("mcp_env"), &store).await
}

async fn migrate_legacy_mcp_env_with_store(
    old_dir: PathBuf,
    store: &FileStore,
) -> crate::Result<()> {
    if tokio::fs::metadata(&old_dir).await.is_err() {
        return Ok(());
    }
    if !old_dir.is_dir() {
        return Ok(());
    }

    let mut migrated = 0usize;
    let mut failed = 0usize;

    let mut entries = match tokio::fs::read_dir(&old_dir).await {
        Ok(e) => e,
        Err(e) => {
            warn!("Cannot read legacy mcp_env dir {}: {e}", old_dir.display());
            return Ok(());
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };

        let legacy: LegacyEnvFile = match tokio::fs::read_to_string(&path).await {
            Ok(content) => match toml::from_str(&content) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Skipping legacy mcp_env {} (unreadable): {e}", path.display());
                    failed += 1;
                    continue;
                }
            },
            Err(_) => continue,
        };

        // Only delete the old file after the new location was written.
        match store.set_all(&id, &legacy.env).await {
            Ok(()) => {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        warn!("Migrated {} but failed to remove legacy file: {e}", path.display());
                    }
                }
                migrated += 1;
            }
            Err(e) => {
                warn!("Failed to migrate legacy mcp_env {}: {e}", path.display());
                failed += 1;
            }
        }
    }

    // Remove the whole old directory once it is empty.
    let remaining = match tokio::fs::read_dir(&old_dir).await {
        Ok(mut e) => {
            let mut n = 0usize;
            while let Ok(Some(_)) = e.next_entry().await {
                n += 1;
            }
            n
        }
        Err(_) => 0,
    };
    if remaining == 0 {
        let _ = tokio::fs::remove_dir(&old_dir).await;
    }

    info!("mcp_env legacy migration: {migrated} migrated, {failed} failed");
    Ok(())
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::store::{SecretId, SecretOrigin};

    /// Unique temp dir per test so concurrent tests do not wipe each other's
    /// state (all tests share the process id, so a name suffix is required).
    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("syscity_file_store_test_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    // Point the store root at the temp dir via an override for tests.
    // We can't easily monkeypatch secrets_root_dir(), so we exercise the
    // public path through a real ~/.syscity/secrets ... instead use the
    // path_for/validate_store on a store whose base we compute from
    // path_for — but that always goes to the real dir. For hermetic tests we
    // write through FileStore but target the temp dir by constructing paths
    // manually via the low-level helpers below.
    async fn write_at(
        dir: &Path,
        entity: &str,
        map: &HashMap<String, String>,
    ) -> crate::Result<()> {
        let path = dir.join(format!("{entity}.toml"));
        tokio::fs::create_dir_all(dir).await?;
        set_dir_perms(dir).await?;
        let content = toml::to_string(&SecretsFile { secrets: map.clone() })?;
        let tmp = path.with_extension("toml.tmp");
        tokio::fs::write(&tmp, content).await?;
        set_file_perms(&tmp).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    async fn read_map(dir: &Path, entity: &str) -> HashMap<String, String> {
        let content = tokio::fs::read_to_string(dir.join(format!("{entity}.toml")))
            .await
            .unwrap_or_default();
        toml::from_str::<SecretsFile>(&content)
            .map(|f| f.secrets)
            .unwrap_or_default()
    }

    #[test]
    fn test_sanitize_entity() {
        assert!(sanitize_entity("github").is_ok());
        assert!(sanitize_entity("github-main").is_ok());
        assert!(sanitize_entity("").is_err());
        assert!(sanitize_entity(".").is_err());
        assert!(sanitize_entity("..").is_err());
        assert!(sanitize_entity("../x").is_err());
        assert!(sanitize_entity("a/b").is_err());
        assert!(sanitize_entity("a\\b").is_err());
    }

    #[tokio::test]
    async fn test_secrets_file_roundtrip() {
        let dir = temp_root("roundtrip");
        let mut map = HashMap::new();
        map.insert("refresh_token".to_string(), "rt_abc".to_string());
        map.insert("client_id".to_string(), "cid".to_string());
        write_at(&dir, "myserver", &map).await.unwrap();

        let loaded = read_map(&dir, "myserver").await;
        assert_eq!(loaded["refresh_token"], "rt_abc");
        assert_eq!(loaded["client_id"], "cid");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_root("perms");
        let mut map = HashMap::new();
        map.insert("k".to_string(), "v".to_string());
        write_at(&dir, "sec", &map).await.unwrap();

        let dir_meta = tokio::fs::metadata(&dir).await.unwrap();
        assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);

        let file_meta = tokio::fs::metadata(dir.join("sec.toml")).await.unwrap();
        assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_secret_store_trait_via_file_store() {
        let root = temp_root("trait").join("root");
        let store = FileStore::with_root("mcp-oauth", root.clone());
        let id = SecretId::new("mcp-oauth", "myserver", "refresh_token");

        store
            .set(&id, "rt_secret", SecretOrigin::UserEntered)
            .await
            .unwrap();
        assert!(store.has(&id).await);
        assert_eq!(store.get(&id).await.unwrap(), Some("rt_secret".to_string()));
        assert!(store.has_entity("myserver").await);
        assert_eq!(store.get_all("myserver").await.unwrap()["refresh_token"], "rt_secret");

        store.delete(&id).await.unwrap();
        assert!(!store.has(&id).await);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_legacy_mcp_env_migration() {
        // Simulate a legacy ~/.syscity/mcp_env with an [env] table.
        let root = temp_root("migration");
        let old_dir = root.join("mcp_env");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        let legacy = "[env]\nGITHUB_PERSONAL_ACCESS_TOKEN = \"ghp_x\"\n";
        tokio::fs::write(old_dir.join("github.toml"), legacy)
            .await
            .unwrap();

        // Migrate into the same temp-rooted store that the read uses, so the
        // test never touches the real ~/.syscity/secrets directory.
        let store = FileStore::with_root("mcp-env", root.join("secrets"));
        migrate_legacy_mcp_env_with_store(old_dir.clone(), &store)
            .await
            .unwrap();

        let loaded = store.get_all("github").await.unwrap();
        assert_eq!(loaded["GITHUB_PERSONAL_ACCESS_TOKEN"], "ghp_x");
        // Old file + directory are gone.
        assert!(!old_dir.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
