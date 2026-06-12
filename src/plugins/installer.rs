//! Plugin Installer
//!
//! Handles downloading plugins from a registry and installing them
//! into the local plugins directory.  Also supports uninstalling.

use std::path::PathBuf;

use crate::plugins::registry::RegistryClient;
use tracing::info;

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
    ///
    /// TODO: actual tar.gz extraction once tar/flate2 are wired in.
    ///       For now the archive bytes are written as a single file.
    pub async fn install(
        &self,
        name: &str,
        registry_url: Option<&str>,
    ) -> crate::Result<()> {
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

        info!(
            "Downloading plugin '{}' v{}...",
            entry.name, entry.version
        );
        let archive = client.download(entry).await?;

        let plugin_dir = self.plugins_dir.join(&entry.name);
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Write the downloaded archive
        let archive_path = plugin_dir.join(format!("{}-{}.tar.gz", entry.name, entry.version));
        tokio::fs::write(&archive_path, &archive).await?;
        info!(
            "Plugin '{}' archive saved to {:?}",
            entry.name, archive_path
        );

        // TODO: extract tar.gz using tar + flate2
        info!("Plugin '{}' installed to {:?}", entry.name, plugin_dir);
        Ok(())
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
