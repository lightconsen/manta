//! SQLite Plugin Registry
//!
//! Provides persistent storage for plugin metadata using SQLite.
//! Tracks installed plugins, their versions, checksums, and enablement state.
//!
//! This module is optional — `PluginManager` works without it.

use std::path::Path;

use serde::Serialize;
use sqlx::sqlite::SqlitePool;

use super::manifest::PluginManifest;

/// Entry in the plugin registry database.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct PluginDbEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub manifest_json: String,
    pub install_path: String,
    pub installed_at: String,
    pub updated_at: String,
    pub enabled: bool,
    pub checksum: Option<String>,
    pub source_url: Option<String>,
}

/// SQLite-backed persistent plugin registry.
pub struct PluginSqliteRegistry {
    pool: SqlitePool,
}

impl PluginSqliteRegistry {
    /// Create a new registry backed by the given SQLite pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Create the registry table if it does not exist.
    pub async fn create_table(&self) -> crate::Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS plugin_registry (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                author TEXT,
                manifest_json TEXT NOT NULL DEFAULT '{}',
                install_path TEXT NOT NULL DEFAULT '',
                installed_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                checksum TEXT,
                source_url TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::SyscityError::Storage {
            context: "plugin_registry".to_string(),
            details: format!("Failed to create plugin_registry table: {}", e),
        })?;
        Ok(())
    }

    /// Register or update a plugin in the registry.
    ///
    /// Uses INSERT OR REPLACE so that re-installing a plugin with the same
    /// `id` updates its metadata.
    pub async fn register_plugin(
        &self,
        manifest: &PluginManifest,
        install_path: &Path,
        checksum: Option<&str>,
    ) -> crate::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let manifest_json =
            serde_json::to_string(manifest).map_err(|e| crate::SyscityError::Serialization(e))?;
        let install_path_str = install_path.to_string_lossy().to_string();

        sqlx::query(
            "INSERT OR REPLACE INTO plugin_registry
                (id, name, version, description, author, manifest_json, install_path,
                 installed_at, updated_at, enabled, checksum)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&manifest.id)
        .bind(&manifest.name)
        .bind(&manifest.version)
        .bind(&manifest.description)
        .bind(&manifest.author)
        .bind(&manifest_json)
        .bind(&install_path_str)
        .bind(&now)
        .bind(&now)
        .bind(checksum)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::SyscityError::Storage {
            context: "plugin_registry".to_string(),
            details: format!("Failed to register plugin '{}': {}", manifest.id, e),
        })?;

        Ok(())
    }

    /// Unregister (delete) a plugin from the registry.
    pub async fn unregister_plugin(&self, id: &str) -> crate::Result<()> {
        sqlx::query("DELETE FROM plugin_registry WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::SyscityError::Storage {
                context: "plugin_registry".to_string(),
                details: format!("Failed to unregister plugin '{}': {}", id, e),
            })?;
        Ok(())
    }

    /// Set the enabled state of a plugin.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> crate::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE plugin_registry SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::SyscityError::Storage {
                context: "plugin_registry".to_string(),
                details: format!("Failed to set enabled for plugin '{}': {}", id, e),
            })?;
        Ok(())
    }

    /// Get a plugin entry by ID.
    pub async fn get_plugin(&self, id: &str) -> crate::Result<Option<PluginDbEntry>> {
        let row = sqlx::query_as::<_, PluginDbEntry>(
            "SELECT id, name, version, description, author, manifest_json, install_path,
                    installed_at, updated_at, enabled, checksum, source_url
             FROM plugin_registry WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::SyscityError::Storage {
            context: "plugin_registry".to_string(),
            details: format!("Failed to get plugin '{}': {}", id, e),
        })?;
        Ok(row)
    }

    /// List all registered plugins.
    pub async fn list_plugins(&self) -> crate::Result<Vec<PluginDbEntry>> {
        let rows = sqlx::query_as::<_, PluginDbEntry>(
            "SELECT id, name, version, description, author, manifest_json, install_path,
                    installed_at, updated_at, enabled, checksum, source_url
             FROM plugin_registry
             ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::SyscityError::Storage {
            context: "plugin_registry".to_string(),
            details: format!("Failed to list plugins: {}", e),
        })?;
        Ok(rows)
    }

    /// Check if a plugin is registered.
    pub async fn plugin_exists(&self, id: &str) -> crate::Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM plugin_registry WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| crate::SyscityError::Storage {
                    context: "plugin_registry".to_string(),
                    details: format!("Failed to check plugin existence '{}': {}", id, e),
                })?;
        Ok(row.map(|r| r.0 > 0).unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn create_test_registry() -> PluginSqliteRegistry {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        let reg = PluginSqliteRegistry::new(pool);
        reg.create_table().await.unwrap();
        reg
    }

    fn test_manifest(id: &str) -> PluginManifest {
        PluginManifest::minimal(id, &format!("Plugin {}", id))
    }

    #[tokio::test]
    async fn test_create_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        let reg = PluginSqliteRegistry::new(pool);
        reg.create_table().await.unwrap();
    }

    #[tokio::test]
    async fn test_register_and_get_plugin() {
        let reg = create_test_registry().await;
        let manifest = test_manifest("com.test.plugin");
        let path = Path::new("/tmp/plugins/test");

        reg.register_plugin(&manifest, path, None).await.unwrap();

        let entry = reg.get_plugin("com.test.plugin").await.unwrap().unwrap();
        assert_eq!(entry.name, "Plugin com.test.plugin");
        assert_eq!(entry.version, "0.1.0");
        assert!(entry.enabled);
    }

    #[tokio::test]
    async fn test_register_with_checksum() {
        let reg = create_test_registry().await;
        let manifest = test_manifest("com.test.checksum");
        let path = Path::new("/tmp/test");

        reg.register_plugin(&manifest, path, Some("sha256:abc123"))
            .await
            .unwrap();

        let entry = reg.get_plugin("com.test.checksum").await.unwrap().unwrap();
        assert_eq!(entry.checksum, Some("sha256:abc123".to_string()));
    }

    #[tokio::test]
    async fn test_unregister_plugin() {
        let reg = create_test_registry().await;
        let manifest = test_manifest("com.test.remove");
        reg.register_plugin(&manifest, Path::new("/tmp"), None)
            .await
            .unwrap();

        reg.unregister_plugin("com.test.remove").await.unwrap();
        let entry = reg.get_plugin("com.test.remove").await.unwrap();
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let reg = create_test_registry().await;
        reg.register_plugin(&test_manifest("com.beta"), Path::new("/tmp"), None)
            .await
            .unwrap();
        reg.register_plugin(&test_manifest("com.alpha"), Path::new("/tmp"), None)
            .await
            .unwrap();

        let list = reg.list_plugins().await.unwrap();
        assert_eq!(list.len(), 2);
        // Should be ordered by name
        assert_eq!(list[0].id, "com.alpha");
        assert_eq!(list[1].id, "com.beta");
    }

    #[tokio::test]
    async fn test_set_enabled() {
        let reg = create_test_registry().await;
        let manifest = test_manifest("com.test.toggle");
        reg.register_plugin(&manifest, Path::new("/tmp"), None)
            .await
            .unwrap();

        reg.set_enabled("com.test.toggle", false).await.unwrap();
        let entry = reg.get_plugin("com.test.toggle").await.unwrap().unwrap();
        assert!(!entry.enabled);

        reg.set_enabled("com.test.toggle", true).await.unwrap();
        let entry = reg.get_plugin("com.test.toggle").await.unwrap().unwrap();
        assert!(entry.enabled);
    }

    #[tokio::test]
    async fn test_plugin_exists() {
        let reg = create_test_registry().await;
        assert!(!reg.plugin_exists("com.test.nonexistent").await.unwrap());

        reg.register_plugin(&test_manifest("com.test.exists"), Path::new("/tmp"), None)
            .await
            .unwrap();
        assert!(reg.plugin_exists("com.test.exists").await.unwrap());
    }

    #[tokio::test]
    async fn test_register_twice_updates() {
        let reg = create_test_registry().await;
        let mut manifest = test_manifest("com.test.update");
        manifest.version = "1.0.0".to_string();
        reg.register_plugin(&manifest, Path::new("/tmp"), None)
            .await
            .unwrap();

        manifest.version = "2.0.0".to_string();
        reg.register_plugin(&manifest, Path::new("/tmp/other"), None)
            .await
            .unwrap();

        let entry = reg.get_plugin("com.test.update").await.unwrap().unwrap();
        assert_eq!(entry.version, "2.0.0");
    }
}
