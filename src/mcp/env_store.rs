//! Secure per-server MCP env-token storage.
//!
//! Tokens the user submits through the Settings UI (e.g. `GITHUB_PERSONAL_ACCESS_TOKEN`)
//! are stored one TOML file per server under `~/.syscity/mcp_env/{server_id}.toml`
//! rather than in config.toml or the process environment. The directory is created
//! with `0700` and each file with `0600` so only the owner can read them.
//!
//! The stored values are literal — they are never run through the `$VAR` expansion
//! used by `McpServerConfig::env`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::SyscityError;

/// `~/.syscity/mcp_env` — home for per-server token files.
pub fn mcp_env_dir() -> PathBuf {
    crate::dirs::syscity_dir().join("mcp_env")
}

/// Path to the env-store file for a server, validating the server id first.
pub fn env_path_for(server_id: &str) -> crate::Result<PathBuf> {
    env_path_for_at(&mcp_env_dir(), server_id)
}

/// Reject server ids that could escape the store directory.
fn sanitize_server_id(server_id: &str) -> crate::Result<String> {
    if server_id.is_empty()
        || server_id == "."
        || server_id == ".."
        || server_id.contains('/')
        || server_id.contains('\\')
        || server_id.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(SyscityError::Validation(format!("invalid MCP server id: {server_id:?}")));
    }
    Ok(server_id.to_string())
}

/// Persist env tokens for a server, creating the store directory if needed.
pub async fn save(server_id: &str, env: &HashMap<String, String>) -> crate::Result<()> {
    save_at(&mcp_env_dir(), server_id, env).await
}

/// Read env tokens back for a server; an empty map if none stored.
pub async fn load(server_id: &str) -> crate::Result<HashMap<String, String>> {
    load_at(&mcp_env_dir(), server_id).await
}

/// Remove a server's env-store file; ignores a missing file.
pub async fn delete(server_id: &str) -> crate::Result<()> {
    delete_at(&mcp_env_dir(), server_id).await
}

/// Whether a server has a stored env file (used for `env_configured`).
pub async fn has(server_id: &str) -> bool {
    has_at(&mcp_env_dir(), server_id).await
}

// ─────────────────────────────────────────────
// Internals (base-dir parameterized for testing)
// ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct EnvFile {
    #[serde(default)]
    env: HashMap<String, String>,
}

fn env_path_for_at(base: &Path, server_id: &str) -> crate::Result<PathBuf> {
    let id = sanitize_server_id(server_id)?;
    Ok(base.join(format!("{id}.toml")))
}

async fn save_at(base: &Path, server_id: &str, env: &HashMap<String, String>) -> crate::Result<()> {
    let path = env_path_for_at(base, server_id)?;
    let dir = path
        .parent()
        .ok_or_else(|| SyscityError::Internal("env store path has no parent".to_string()))?;
    tokio::fs::create_dir_all(dir).await?;
    set_dir_perms(dir).await?;

    let content = toml::to_string(&EnvFile { env: env.clone() })?;
    // Write to a temp file, tighten permissions, then atomically rename so the
    // final file never carries default (wider) permissions.
    let tmp = path.with_extension("toml.tmp");
    tokio::fs::write(&tmp, content).await?;
    set_file_perms(&tmp).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

async fn load_at(base: &Path, server_id: &str) -> crate::Result<HashMap<String, String>> {
    let path = env_path_for_at(base, server_id)?;
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let file: EnvFile = toml::from_str(&content)?;
            Ok(file.env)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

async fn delete_at(base: &Path, server_id: &str) -> crate::Result<()> {
    let path = env_path_for_at(base, server_id)?;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn has_at(base: &Path, server_id: &str) -> bool {
    match env_path_for_at(base, server_id) {
        Ok(path) => tokio::fs::metadata(path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false),
        Err(_) => false,
    }
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
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_base() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("syscity_env_store_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn test_sanitize_server_id() {
        assert!(sanitize_server_id("github").is_ok());
        assert!(sanitize_server_id("github-main").is_ok());
        assert!(sanitize_server_id("").is_err());
        assert!(sanitize_server_id("..").is_err());
        assert!(sanitize_server_id(".").is_err());
        assert!(sanitize_server_id("../x").is_err());
        assert!(sanitize_server_id("a/b").is_err());
        assert!(sanitize_server_id("a\\b").is_err());
    }

    #[tokio::test]
    async fn test_save_load_delete_roundtrip() {
        let base = temp_base();
        let mut env = HashMap::new();
        env.insert("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), "ghp_abc".to_string());
        env.insert("FOO".to_string(), "bar".to_string());

        save_at(&base, "github", &env).await.unwrap();
        assert!(has_at(&base, "github").await);

        let loaded = load_at(&base, "github").await.unwrap();
        assert_eq!(loaded["GITHUB_PERSONAL_ACCESS_TOKEN"], "ghp_abc");
        assert_eq!(loaded["FOO"], "bar");

        // Missing server loads as empty.
        assert!(load_at(&base, "ghost").await.unwrap().is_empty());

        delete_at(&base, "github").await.unwrap();
        assert!(!has_at(&base, "github").await);

        // Deleting a missing file is not an error.
        delete_at(&base, "github").await.unwrap();

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let base = temp_base();

        let mut env = HashMap::new();
        env.insert("K".to_string(), "v".to_string());
        save_at(&base, "sec", &env).await.unwrap();

        let dir_meta = tokio::fs::metadata(&base).await.unwrap();
        assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);

        let file_meta = tokio::fs::metadata(env_path_for_at(&base, "sec").unwrap())
            .await
            .unwrap();
        assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_env_path_for_rejects_bad_ids() {
        assert!(env_path_for("ok").is_ok());
        assert!(env_path_for("../escape").is_err());
        assert!(env_path_for("").is_err());
        assert!(env_path_for("a/b").is_err());
    }
}
