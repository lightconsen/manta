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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_new_installer() {
        let tmp = tempdir().unwrap();
        let installer = PluginInstaller::new(tmp.path().to_path_buf());
        assert_eq!(installer.plugins_dir, tmp.path());
    }

    #[tokio::test]
    async fn test_extract_archive_valid_tar_gz() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("extracted");
        tokio::fs::create_dir_all(&target).await.unwrap();

        // Create a valid tar.gz archive in memory
        let mut tar_builder = tar::Builder::new(Vec::new());
        tar_builder
            .append_dir("plugins/test-plugin", std::path::Path::new("."))
            .unwrap();
        let test_content = b"hello, world!";
        let mut header = tar::Header::new_gnu();
        header.set_path("plugins/test-plugin/hello.txt").unwrap();
        header.set_size(test_content.len() as u64);
        header.set_cksum();
        tar_builder.append(&header, &test_content[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        // Gzip compress
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        // Write to disk and extract
        let archive_path = tmp.path().join("test.tar.gz");
        tokio::fs::write(&archive_path, &gz_bytes).await.unwrap();
        PluginInstaller::extract_archive(&archive_path, &target)
            .await
            .unwrap();

        // Verify extraction
        let extracted_file = target.join("plugins/test-plugin/hello.txt");
        let content = tokio::fs::read_to_string(&extracted_file).await.unwrap();
        assert_eq!(content, "hello, world!");
    }

    /// Build a raw tar.gz with a single entry at the given path.
    /// Uses raw tar header bytes so we can inject malicious paths that
    /// `tar::Builder` rejects.
    fn build_raw_tar_gz(path: &str, content: &[u8]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        // Build a 512-byte tar header, then set fields.
        let mut hdr = [0u8; 512];

        // Name field (bytes 0-99)
        let name_bytes = path.as_bytes();
        let len = name_bytes.len().min(99);
        hdr[..len].copy_from_slice(&name_bytes[..len]);

        // Mode (100-107)
        hdr[100..107].copy_from_slice(b"0000644");

        // UID (108-115)
        hdr[108..115].copy_from_slice(b"0000000");

        // GID (116-123)
        hdr[116..123].copy_from_slice(b"0000000");

        // Size (124-135) — octal
        let size_str = format!("{:011o}", content.len());
        hdr[124..135].copy_from_slice(size_str.as_bytes());

        // Mtime (136-147)
        hdr[136..147].copy_from_slice(b"00000000000");

        // Type flag (156) — '0' = regular file
        hdr[156] = b'0';

        // Compute checksum: sum of all bytes in hdr, treating
        // bytes 148-155 (the checksum field) as spaces.
        let saved_chk = hdr[148..156].to_vec();
        hdr[148..156].copy_from_slice(b"        ");
        let cksum: u32 = hdr.iter().map(|&b| b as u32).sum();
        let cksum_str = format!("{:06o}\0 ", cksum);
        hdr[148..156].copy_from_slice(cksum_str.as_bytes());

        let mut tar_bytes = hdr.to_vec();

        // Content padded to 512-byte block
        tar_bytes.extend_from_slice(content);
        let padding = (512 - (content.len() % 512)) % 512;
        tar_bytes.extend_from_slice(&vec![0u8; padding]);

        // Gzip compress
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[tokio::test]
    async fn test_extract_archive_zip_slip_parent() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("extracted");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let gz_bytes = build_raw_tar_gz("../evil.txt", b"malicious");
        let archive_path = tmp.path().join("zip-slip.tar.gz");
        tokio::fs::write(&archive_path, &gz_bytes).await.unwrap();
        let result = PluginInstaller::extract_archive(&archive_path, &target).await;

        assert!(result.is_err(), "expected error but got Ok");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Zip-slip") || err.contains("unsafe path"),
            "error '{}' does not mention zip-slip or unsafe path",
            err
        );
    }

    #[tokio::test]
    async fn test_extract_archive_zip_slip_absolute() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("extracted");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let gz_bytes = build_raw_tar_gz("/etc/passwd", b"malicious");
        let archive_path = tmp.path().join("absolute.tar.gz");
        tokio::fs::write(&archive_path, &gz_bytes).await.unwrap();
        let result = PluginInstaller::extract_archive(&archive_path, &target).await;

        assert!(result.is_err(), "expected error but got Ok");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Zip-slip") || err.contains("unsafe path"),
            "error '{}' does not mention zip-slip or unsafe path",
            err
        );
    }

    #[tokio::test]
    async fn test_uninstall_not_found() {
        let tmp = tempdir().unwrap();
        let installer = PluginInstaller::new(tmp.path().to_path_buf());
        let result = installer.uninstall("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_uninstall_success() {
        let tmp = tempdir().unwrap();
        let plugin_dir = tmp.path().join("test-plugin");
        tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
        tokio::fs::write(plugin_dir.join("file.txt"), b"data")
            .await
            .unwrap();

        let installer = PluginInstaller::new(tmp.path().to_path_buf());
        let result = installer.uninstall("test-plugin").await;
        assert!(result.is_ok());
        assert!(!plugin_dir.exists());
    }
}
