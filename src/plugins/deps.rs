//! Plugin Dependency Resolution
//!
//! Resolves plugin dependencies in topological order and handles
//! downloading external resources referenced by plugin manifests.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::Digest;
use tracing::{debug, info, warn};

use crate::plugins::manifest::PluginManifest;

/// A resolved dependency entry.
#[derive(Debug, Clone)]
pub struct ResolvedDependency {
    /// Plugin ID
    pub id: String,
    /// Installed version
    pub version: String,
    /// Path to the plugin directory on disk
    pub path: PathBuf,
}

/// Resolves plugin dependencies and ensures external resources are available.
pub struct DependencyResolver {
    plugins_dir: PathBuf,
    http_client: reqwest::Client,
}

impl DependencyResolver {
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            http_client: reqwest::Client::new(),
        }
    }

    /// Resolve all dependencies of a plugin, returning them in topological
    /// order (dependencies before dependents).
    ///
    /// Uses iterative DFS with explicit stack to avoid lifetime issues with
    /// recursive boxed futures.
    pub async fn resolve(&self, plugin_id: &str) -> crate::Result<Vec<ResolvedDependency>> {
        let mut resolved = Vec::new();
        let mut visited = HashSet::new();
        let mut in_progress = HashSet::new();
        // Stack entries: (plugin_id, children_processed_flag)
        let mut stack: Vec<(String, bool)> = vec![(plugin_id.to_string(), false)];

        while let Some((id, children_processed)) = stack.pop() {
            if children_processed {
                // All deps processed; add this plugin to the result
                in_progress.remove(&id);
                visited.insert(id.clone());

                let plugin_dir = self.plugins_dir.join(&id);
                let manifest_path = plugin_dir.join("plugin.json");
                if let Ok(content) = tokio::fs::read_to_string(&manifest_path).await {
                    if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&content) {
                        resolved.push(ResolvedDependency {
                            id: manifest.id.clone(),
                            version: manifest.version.clone(),
                            path: plugin_dir,
                        });
                    }
                }
                continue;
            }

            if visited.contains(&id) {
                continue;
            }
            if !in_progress.insert(id.clone()) {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Cyclic dependency detected involving '{}'",
                    id
                )));
            }

            // Push post-order marker
            stack.push((id.clone(), true));

            // Load manifest and push dependencies
            let manifest_path = self.plugins_dir.join(&id).join("plugin.json");
            if let Ok(content) = tokio::fs::read_to_string(&manifest_path).await {
                if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&content) {
                    if let Some(ref deps) = manifest.dependencies {
                        for dep_id in deps.keys() {
                            if !visited.contains(dep_id) {
                                stack.push((dep_id.clone(), false));
                            }
                        }
                    }
                }
            }
        }

        Ok(resolved)
    }

    /// Ensure that all external resources declared by a plugin are available.
    ///
    /// Downloads any missing required resources and verifies checksums
    /// for existing ones.
    pub async fn ensure_resources(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> crate::Result<()> {
        let resources = match manifest.external_resources {
            Some(ref r) => r,
            None => return Ok(()),
        };

        for resource in resources {
            let target_path = plugin_dir.join(&resource.path);
            if target_path.exists() {
                // Verify checksum if provided
                if let Some(ref checksum) = resource.checksum_sha256 {
                    let content = tokio::fs::read(&target_path).await?;
                    let actual = hex::encode(sha2::Sha256::digest(&content));
                    if actual != *checksum {
                        if resource.required {
                            return Err(crate::error::SyscityError::Internal(format!(
                                "Checksum mismatch for {:?}",
                                target_path
                            )));
                        }
                        warn!("Checksum mismatch for {:?}, but resource not required", target_path);
                    }
                }
                continue;
            }

            if !resource.required {
                debug!(
                    "Skipping optional resource {} (not required, target: {:?})",
                    resource.url, target_path
                );
                continue;
            }

            info!("Downloading external resource {} -> {:?}", resource.url, target_path);
            if let Some(parent) = target_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let resp = self.http_client.get(&resource.url).send().await?;
            let bytes = resp.bytes().await?;

            if let Some(ref checksum) = resource.checksum_sha256 {
                let actual = hex::encode(sha2::Sha256::digest(&bytes));
                if actual != *checksum {
                    return Err(crate::error::SyscityError::Internal(format!(
                        "Checksum mismatch for downloaded resource {}",
                        resource.url
                    )));
                }
            }

            tokio::fs::write(&target_path, &bytes).await?;
            info!("Downloaded {} -> {:?}", resource.url, target_path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_resolve_no_deps() {
        let tmp = tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        tokio::fs::create_dir_all(&plugins_dir).await.unwrap();

        let manifest = PluginManifest::minimal("com.test.standalone", "Standalone");
        let plugin_dir = plugins_dir.join("com.test.standalone");
        tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
        tokio::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let resolver = DependencyResolver::new(plugins_dir);
        let deps = resolver.resolve("com.test.standalone").await.unwrap();
        // Should resolve to itself (no dependencies)
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].id, "com.test.standalone");
    }
}
