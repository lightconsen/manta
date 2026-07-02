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
            http_client: Self::build_http_client(),
        }
    }

    /// Build an HTTP client with a 30-second timeout.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
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
    use std::collections::HashMap;
    use tempfile::tempdir;

    use super::*;
    use crate::plugins::manifest::ExternalResource;

    fn write_manifest(dir: &std::path::Path, manifest: &PluginManifest) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(manifest).unwrap())
            .unwrap();
    }

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

    #[tokio::test]
    async fn test_resolve_one_dep() {
        let tmp = tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");

        // Plugin B (dependency)
        let mut deps_b = HashMap::new();
        deps_b.insert("com.dep.lib".to_string(), "1".to_string());
        let manifest_b = PluginManifest {
            id: "com.dep.lib".to_string(),
            name: "Lib".to_string(),
            version: "1.0.0".to_string(),
            dependencies: None,
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.dep.lib"), &manifest_b);

        // Plugin A (depends on B)
        let mut deps_a = HashMap::new();
        deps_a.insert("com.dep.lib".to_string(), "1".to_string());
        let manifest_a = PluginManifest {
            id: "com.test.dep".to_string(),
            name: "Dep".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_a),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.test.dep"), &manifest_a);

        let resolver = DependencyResolver::new(plugins_dir);
        let deps = resolver.resolve("com.test.dep").await.unwrap();
        // Should resolve to: dep first, then the plugin itself
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].id, "com.dep.lib");
        assert_eq!(deps[1].id, "com.test.dep");
    }

    #[tokio::test]
    async fn test_resolve_chain() {
        let tmp = tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");

        // Plugin C (leaf)
        let manifest_c = PluginManifest {
            id: "com.chain.c".to_string(),
            name: "C".to_string(),
            version: "1.0.0".to_string(),
            dependencies: None,
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.chain.c"), &manifest_c);

        // Plugin B (depends on C)
        let mut deps_b = HashMap::new();
        deps_b.insert("com.chain.c".to_string(), "1".to_string());
        let manifest_b = PluginManifest {
            id: "com.chain.b".to_string(),
            name: "B".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_b),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.chain.b"), &manifest_b);

        // Plugin A (depends on B)
        let mut deps_a = HashMap::new();
        deps_a.insert("com.chain.b".to_string(), "1".to_string());
        let manifest_a = PluginManifest {
            id: "com.chain.a".to_string(),
            name: "A".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_a),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.chain.a"), &manifest_a);

        let resolver = DependencyResolver::new(plugins_dir);
        let deps = resolver.resolve("com.chain.a").await.unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].id, "com.chain.c");
        assert_eq!(deps[1].id, "com.chain.b");
        assert_eq!(deps[2].id, "com.chain.a");
    }

    #[tokio::test]
    async fn test_resolve_cycle_detected() {
        let tmp = tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");

        // Plugin A (depends on B)
        let mut deps_a = HashMap::new();
        deps_a.insert("com.cycle.b".to_string(), "1".to_string());
        let manifest_a = PluginManifest {
            id: "com.cycle.a".to_string(),
            name: "A".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_a),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.cycle.a"), &manifest_a);

        // Plugin B (depends on A — cycle!)
        let mut deps_b = HashMap::new();
        deps_b.insert("com.cycle.a".to_string(), "1".to_string());
        let manifest_b = PluginManifest {
            id: "com.cycle.b".to_string(),
            name: "B".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_b),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.cycle.b"), &manifest_b);

        let resolver = DependencyResolver::new(plugins_dir);
        let result = resolver.resolve("com.cycle.a").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cyclic dependency"));
    }

    #[tokio::test]
    async fn test_resolve_missing_dep_skipped() {
        // When a dependency's manifest is missing, it's silently skipped
        let tmp = tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");

        // Plugin A (depends on B, but B has no plugin.json)
        let mut deps_a = HashMap::new();
        deps_a.insert("com.missing.b".to_string(), "1".to_string());
        let manifest_a = PluginManifest {
            id: "com.missing.a".to_string(),
            name: "A".to_string(),
            version: "1.0.0".to_string(),
            dependencies: Some(deps_a),
            ..PluginManifest::minimal("", "")
        };
        write_manifest(&plugins_dir.join("com.missing.a"), &manifest_a);

        // Create B directory but no manifest
        std::fs::create_dir_all(plugins_dir.join("com.missing.b")).unwrap();

        let resolver = DependencyResolver::new(plugins_dir);
        let deps = resolver.resolve("com.missing.a").await.unwrap();
        // Only A is resolved; B is skipped
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].id, "com.missing.a");
    }

    #[tokio::test]
    async fn test_ensure_resources_no_resources() {
        let tmp = tempdir().unwrap();
        let resolver = DependencyResolver::new(tmp.path().to_path_buf());
        let manifest = PluginManifest::minimal("com.test.none", "None");
        let result = resolver.ensure_resources(&manifest, tmp.path()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_resources_missing_optional_skipped() {
        let tmp = tempdir().unwrap();
        let resolver = DependencyResolver::new(tmp.path().to_path_buf());
        let resource = ExternalResource {
            url: "https://example.com/missing.txt".to_string(),
            path: "missing.txt".to_string(),
            checksum_sha256: None,
            required: false,
        };
        let manifest = PluginManifest {
            external_resources: Some(vec![resource]),
            ..PluginManifest::minimal("com.test.opt", "Optional")
        };
        // Should succeed (optional resource not downloaded, no error)
        let result = resolver.ensure_resources(&manifest, tmp.path()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_resources_existing_verified() {
        let tmp = tempdir().unwrap();
        let content = b"existing file content";
        let target_path = tmp.path().join("existing.txt");
        tokio::fs::write(&target_path, content).await.unwrap();

        let checksum = hex::encode(sha2::Sha256::digest(content));

        let resolver = DependencyResolver::new(tmp.path().to_path_buf());
        let resource = ExternalResource {
            url: "https://example.com/existing.txt".to_string(),
            path: "existing.txt".to_string(),
            checksum_sha256: Some(checksum),
            required: true,
        };
        let manifest = PluginManifest {
            external_resources: Some(vec![resource]),
            ..PluginManifest::minimal("com.test.checksum", "Checksum")
        };
        let result = resolver.ensure_resources(&manifest, tmp.path()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_resources_checksum_mismatch_required() {
        let tmp = tempdir().unwrap();
        let content = b"original content";
        let target_path = tmp.path().join("mismatch.txt");
        tokio::fs::write(&target_path, content).await.unwrap();

        // Wrong checksum
        let bad_checksum = hex::encode(sha2::Sha256::digest(b"different content"));

        let resolver = DependencyResolver::new(tmp.path().to_path_buf());
        let resource = ExternalResource {
            url: "https://example.com/mismatch.txt".to_string(),
            path: "mismatch.txt".to_string(),
            checksum_sha256: Some(bad_checksum),
            required: true,
        };
        let manifest = PluginManifest {
            external_resources: Some(vec![resource]),
            ..PluginManifest::minimal("com.test.badsum", "BadSum")
        };
        let result = resolver.ensure_resources(&manifest, tmp.path()).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Checksum mismatch"));
    }

    #[tokio::test]
    async fn test_ensure_resources_checksum_mismatch_optional() {
        let tmp = tempdir().unwrap();
        let content = b"original content";
        let target_path = tmp.path().join("opt_mismatch.txt");
        tokio::fs::write(&target_path, content).await.unwrap();

        let bad_checksum = hex::encode(sha2::Sha256::digest(b"different content"));

        let resolver = DependencyResolver::new(tmp.path().to_path_buf());
        let resource = ExternalResource {
            url: "https://example.com/opt_mismatch.txt".to_string(),
            path: "opt_mismatch.txt".to_string(),
            checksum_sha256: Some(bad_checksum),
            required: false,
        };
        let manifest = PluginManifest {
            external_resources: Some(vec![resource]),
            ..PluginManifest::minimal("com.test.optbad", "OptBad")
        };
        // Should succeed with warning (not required)
        let result = resolver.ensure_resources(&manifest, tmp.path()).await;
        assert!(result.is_ok());
    }
}
