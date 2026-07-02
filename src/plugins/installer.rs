//! Plugin Installer
//!
//! Handles downloading plugins from a registry and installing them
//! into the local plugins directory.  Also supports uninstalling.

use std::path::PathBuf;

use tracing::{debug, info};

use crate::plugins::registry::RegistryClient;

/// Installs and uninstalls plugins from remote registries.
pub struct PluginInstaller {
    plugins_dir: PathBuf,
}

impl PluginInstaller {
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self { plugins_dir }
    }

    /// Install a plugin by name from a registry.
    ///
    /// Looks up the plugin in the registry index, downloads the archive,
    /// and extracts it into `plugins_dir/{name}`.
    pub async fn install(&self, name: &str, registry_url: Option<&str>) -> crate::Result<()> {
        let url = registry_url.unwrap_or("https://plugins.syscity.dev");
        let client = RegistryClient::new(url);
        let index = client.fetch_index().await?;

        let entry = index
            .plugins
            .iter()
            .find(|p| p.id == name || p.name == name)
            .ok_or_else(|| {
                crate::error::SyscityError::Internal(format!(
                    "Plugin '{}' not found in registry",
                    name
                ))
            })?;

        info!("Downloading plugin '{}' v{}...", entry.name, entry.version);
        let archive = client.download(entry).await?;

        let plugin_dir = self.plugins_dir.join(&entry.name);
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Write the downloaded archive to a temp file
        let archive_name = format!("{}-{}.tar.gz", entry.name, entry.version);
        let archive_path = plugin_dir.join(&archive_name);
        tokio::fs::write(&archive_path, &archive).await?;
        debug!("Plugin archive saved to {:?}", archive_path);

        // Extract tar.gz using tar + flate2
        Self::extract_archive(&archive_path, &plugin_dir).await?;

        // Remove the archive after successful extraction
        tokio::fs::remove_file(&archive_path).await?;
        debug!("Removed archive {:?} after extraction", archive_path);

        info!("Plugin '{}' installed to {:?}", entry.name, plugin_dir);
        Ok(())
    }

    /// Extract a .tar.gz archive into the target directory.
    async fn extract_archive(
        archive_path: &std::path::Path,
        target_dir: &std::path::Path,
    ) -> crate::Result<()> {
        let archive_bytes = tokio::fs::read(archive_path).await?;

        // Decode gzip and unpack tar in a blocking task (I/O heavy)
        tokio::task::spawn_blocking({
            let target = target_dir.to_path_buf();
            move || -> crate::Result<()> {
                let decoder =
                    flate2::read::GzDecoder::new(std::io::BufReader::new(&archive_bytes[..]));
                let mut archive = tar::Archive::new(decoder);

                // Unpack each entry, sanitising paths to prevent zip-slip
                for entry in archive.entries().map_err(|e| {
                    crate::error::SyscityError::Internal(format!(
                        "Failed to read tar entries: {}",
                        e
                    ))
                })? {
                    let mut entry = entry.map_err(|e| {
                        crate::error::SyscityError::Internal(format!(
                            "Failed to read tar entry: {}",
                            e
                        ))
                    })?;

                    // Path sanitisation: reject entries with absolute paths or
                    // parent-directory traversal.
                    let raw_path = entry.path().map_err(|e| {
                        crate::error::SyscityError::Internal(format!(
                            "Failed to get tar entry path: {}",
                            e
                        ))
                    })?;
                    let components: Vec<_> = raw_path.components().collect();
                    if components.iter().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    }) {
                        return Err(crate::error::SyscityError::Internal(format!(
                            "Zip-slip detected: tar entry '{}' contains unsafe path components",
                            raw_path.display()
                        )));
                    }

                    // Clone the path before unpack_in to avoid borrow conflict
                    let raw_path_clone = raw_path.to_path_buf();
                    entry.unpack_in(&target).map_err(|e| {
                        crate::error::SyscityError::Internal(format!(
                            "Failed to extract tar entry '{}': {}",
                            raw_path_clone.display(),
                            e
                        ))
                    })?;
                }

                info!("Extracted archive to {:?}", target);
                Ok(())
            }
        })
        .await
        .map_err(|e| {
            crate::error::SyscityError::Internal(format!("Extraction task failed: {}", e))
        })?
    }

    /// Remove an installed plugin by name.
    pub async fn uninstall(&self, name: &str) -> crate::Result<()> {
        let plugin_dir = self.plugins_dir.join(name);
        if !plugin_dir.exists() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Plugin '{}' not found at {:?}",
                name, plugin_dir
            )));
        }
        tokio::fs::remove_dir_all(&plugin_dir).await?;
        info!("Plugin '{}' uninstalled", name);
        Ok(())
    }
}
