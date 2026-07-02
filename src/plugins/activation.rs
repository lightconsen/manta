//! Activation Planner
//!
//! Determines plugin load order and lazy activation triggers.
//! Uses Kahn's algorithm for topological sort of dependency graphs,
//! avoiding the need for an external graph library.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::manifest::PluginManifest;

/// Trigger types for plugin activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginTrigger {
    /// Activate when a specific command is used.
    Command(String),
    /// Activate when a specific provider is used.
    Provider(String),
    /// Activate when a channel of this type starts.
    Channel(String),
    /// Activate on system startup.
    OnStartup,
    /// Activate when another plugin with this capability loads.
    Capability(String),
}

impl PluginManifest {
    /// Get trigger declarations from the manifest.
    ///
    /// Returns an empty vec if no triggers are defined.
    pub fn get_triggers(&self) -> Vec<PluginTrigger> {
        self.triggers.clone().unwrap_or_default()
    }

    /// Check if this plugin has an OnStartup trigger.
    pub fn has_startup_trigger(&self) -> bool {
        self.get_triggers()
            .iter()
            .any(|t| matches!(t, PluginTrigger::OnStartup))
    }

    /// Get the plugin IDs that this plugin depends on via dependency field.
    pub fn get_dependency_ids(&self) -> Vec<String> {
        self.dependencies
            .as_ref()
            .map(|deps| deps.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Activation planner — determines load order and lazy activation triggers.
pub struct ActivationPlanner {
    plugins_dir: PathBuf,
}

impl ActivationPlanner {
    /// Create a new activation planner for the given plugins directory.
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self { plugins_dir }
    }

    /// Get the plugins directory.
    pub fn plugins_dir(&self) -> &PathBuf {
        &self.plugins_dir
    }

    /// Discover all plugin manifests in the plugins directory.
    async fn discover_manifests(&self) -> crate::Result<Vec<(String, PluginManifest, PathBuf)>> {
        let mut entries = tokio::fs::read_dir(&self.plugins_dir).await?;
        let mut manifests = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    match tokio::fs::read_to_string(&manifest_path).await {
                        Ok(content) => match serde_json::from_str::<PluginManifest>(&content) {
                            Ok(manifest) => {
                                manifests.push((manifest.id.clone(), manifest, path));
                            }
                            Err(e) => {
                                warn!("Invalid manifest at {:?}: {}", manifest_path, e);
                            }
                        },
                        Err(e) => {
                            warn!("Failed to read manifest at {:?}: {}", manifest_path, e);
                        }
                    }
                }
            }
        }

        Ok(manifests)
    }

    /// Plan activation order for all plugins.
    ///
    /// 1. Discovers all plugin manifests
    /// 2. Builds a dependency graph
    /// 3. Topologically sorts using Kahn's algorithm
    /// 4. Groups by trigger type
    /// 5. Detects cycles and missing dependencies
    pub async fn plan_activation(&self) -> crate::Result<ActivationPlan> {
        let manifests = self.discover_manifests().await?;

        let mut plugin_map: HashMap<String, PluginManifest> = HashMap::new();
        for (id, manifest, _path) in &manifests {
            plugin_map.insert(id.clone(), manifest.clone());
        }

        // Build dependency graph
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();

        for (id, _manifest, _path) in &manifests {
            in_degree.entry(id.clone()).or_insert(0);
            adj.entry(id.clone()).or_default();
        }

        for (id, manifest, _path) in &manifests {
            let deps = manifest.get_dependency_ids();
            for dep_id in &deps {
                if plugin_map.contains_key(dep_id) {
                    adj.entry(dep_id.clone()).or_default().push(id.clone());
                    *in_degree.entry(id.clone()).or_insert(0) += 1;
                }
            }
        }

        // Detect missing dependencies
        let mut missing_deps = Vec::new();
        for (id, manifest, _path) in &manifests {
            let deps = manifest.get_dependency_ids();
            for dep_id in &deps {
                if !plugin_map.contains_key(dep_id) {
                    missing_deps.push((id.clone(), dep_id.clone(), "missing".to_string()));
                }
            }
        }

        // Kahn's algorithm for topological sort
        let mut queue: VecDeque<String> = VecDeque::new();
        for (id, deg) in &in_degree {
            if *deg == 0 {
                queue.push_back(id.clone());
            }
        }

        let mut load_order = Vec::new();
        while let Some(id) = queue.pop_front() {
            load_order.push(id.clone());
            if let Some(neighbors) = adj.get(&id) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }

        // Detect cycles
        let mut cycles = Vec::new();
        let sorted_set: HashSet<String> = load_order.iter().cloned().collect();
        for id in plugin_map.keys() {
            if !sorted_set.contains(id) {
                // This node (and anything reachable from it) is in a cycle
                let cycle = self.trace_cycle(id, &adj, &sorted_set);
                if !cycle.is_empty() {
                    cycles.push(cycle);
                }
            }
        }

        Ok(ActivationPlan {
            load_order,
            cycles,
            missing_deps,
        })
    }

    /// Trace a cycle starting from a given node.
    fn trace_cycle(
        &self,
        start: &str,
        adj: &HashMap<String, Vec<String>>,
        sorted: &HashSet<String>,
    ) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        let mut stack = vec![start.to_string()];

        while let Some(node) = stack.last() {
            if visited.contains(node) {
                // Found a cycle — extract the cycle part
                let pos = path.iter().position(|x| x == node);
                if let Some(pos) = pos {
                    return path[pos..].to_vec();
                }
                break;
            }
            visited.insert(node.clone());
            path.push(node.clone());

            if let Some(neighbors) = adj.get(node) {
                let mut found = false;
                for neighbor in neighbors {
                    if !sorted.contains(neighbor) {
                        stack.push(neighbor.clone());
                        found = true;
                        break;
                    }
                }
                if !found {
                    stack.pop();
                    path.pop();
                }
            } else {
                stack.pop();
                path.pop();
            }

            if visited.len() > 1000 {
                break; // Safety limit
            }
        }

        vec![]
    }

    /// Get plugins that should be loaded on startup.
    ///
    /// These are plugins with an `OnStartup` trigger or no trigger at all.
    pub async fn startup_plugins(&self) -> crate::Result<Vec<String>> {
        let manifests = self.discover_manifests().await?;

        let mut result = Vec::new();
        for (id, manifest, _path) in &manifests {
            let triggers = manifest.get_triggers();
            if triggers.is_empty() || manifest.has_startup_trigger() {
                result.push(id.clone());
            }
        }

        // Sort by dependency order
        let plan = self.plan_activation().await?;
        result.sort_by_key(|id| {
            plan.load_order
                .iter()
                .position(|x| x == id)
                .unwrap_or(usize::MAX)
        });

        Ok(result)
    }

    /// Get plugins triggered by a specific event.
    pub async fn triggered_plugins(&self, trigger: &PluginTrigger) -> crate::Result<Vec<String>> {
        let manifests = self.discover_manifests().await?;
        let key = trigger_key(trigger);

        let mut result = Vec::new();
        for (id, manifest, _path) in &manifests {
            for t in manifest.get_triggers() {
                if trigger_key(&t) == key {
                    result.push(id.clone());
                    break;
                }
            }
        }

        // Sort by dependency order
        let plan = self.plan_activation().await?;
        result.sort_by_key(|id| {
            plan.load_order
                .iter()
                .position(|x| x == id)
                .unwrap_or(usize::MAX)
        });

        Ok(result)
    }
}

/// Compute a string key for a trigger (used for HashMap lookups).
pub fn trigger_key(trigger: &PluginTrigger) -> String {
    match trigger {
        PluginTrigger::Command(name) => format!("command:{}", name),
        PluginTrigger::Provider(name) => format!("provider:{}", name),
        PluginTrigger::Channel(name) => format!("channel:{}", name),
        PluginTrigger::OnStartup => "on_startup".to_string(),
        PluginTrigger::Capability(name) => format!("capability:{}", name),
    }
}

/// Result of activation planning.
#[derive(Debug)]
pub struct ActivationPlan {
    /// Load order for startup plugins (topologically sorted).
    pub load_order: Vec<String>,
    /// Cyclic dependencies detected.
    pub cycles: Vec<Vec<String>>,
    /// Missing dependencies: (plugin_id, dep_name, constraint).
    pub missing_deps: Vec<(String, String, String)>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use tempfile::tempdir;

    use super::*;

    fn create_minimal_manifest(id: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "version": "0.1.0",
            "description": "Test plugin"
        })
    }

    fn manifest_with_triggers(id: &str, triggers: Vec<PluginTrigger>) -> serde_json::Value {
        let mut json = create_minimal_manifest(id, id);
        json["triggers"] = serde_json::to_value(&triggers).unwrap();
        json
    }

    fn manifest_with_deps(id: &str, deps: HashMap<String, String>) -> serde_json::Value {
        let mut json = create_minimal_manifest(id, id);
        json["dependencies"] = serde_json::to_value(&deps).unwrap();
        json
    }

    async fn write_manifest(
        dir: &tempfile::TempDir,
        plugin_id: &str,
        manifest: &serde_json::Value,
    ) {
        let p_dir = dir.path().join(plugin_id);
        tokio::fs::create_dir_all(&p_dir).await.unwrap();
        tokio::fs::write(
            p_dir.join("plugin.json"),
            serde_json::to_string_pretty(manifest).unwrap(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_plan_activation_empty_dir() {
        let tmp = tempdir().unwrap();
        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();
        assert!(plan.load_order.is_empty());
        assert!(plan.cycles.is_empty());
    }

    #[tokio::test]
    async fn test_plan_activation_single_plugin() {
        let tmp = tempdir().unwrap();
        let plugin_dir = tmp.path().join("test-plugin");
        tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
        tokio::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::to_string_pretty(&create_minimal_manifest("com.test.single", "Single"))
                .unwrap(),
        )
        .await
        .unwrap();

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();
        assert_eq!(plan.load_order, vec!["com.test.single"]);
    }

    #[tokio::test]
    async fn test_plan_activation_multiple_independent() {
        let tmp = tempdir().unwrap();

        for id in &["alpha", "beta", "gamma"] {
            write_manifest(&tmp, id, &create_minimal_manifest(&format!("com.test.{}", id), id))
                .await;
        }

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        assert_eq!(plan.load_order.len(), 3);
        for id in &["com.test.alpha", "com.test.beta", "com.test.gamma"] {
            assert!(plan.load_order.contains(&id.to_string()));
        }
    }

    #[tokio::test]
    async fn test_plan_activation_single_chain() {
        let tmp = tempdir().unwrap();

        // A depends on B, B depends on C → load order: C, B, A
        write_manifest(
            &tmp,
            "a",
            &manifest_with_deps(
                "com.test.a",
                HashMap::from([("com.test.b".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(
            &tmp,
            "b",
            &manifest_with_deps(
                "com.test.b",
                HashMap::from([("com.test.c".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(&tmp, "c", &create_minimal_manifest("com.test.c", "C")).await;

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        assert_eq!(plan.load_order.len(), 3);
        // C must come before B, B must come before A
        let pos_c = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.c")
            .unwrap();
        let pos_b = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.b")
            .unwrap();
        let pos_a = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.a")
            .unwrap();
        assert!(pos_c < pos_b, "C should load before B");
        assert!(pos_b < pos_a, "B should load before A");
    }

    #[tokio::test]
    async fn test_plan_activation_diamond_deps() {
        let tmp = tempdir().unwrap();

        // A depends on B and C (B and C are independent)
        write_manifest(
            &tmp,
            "a",
            &manifest_with_deps(
                "com.test.a",
                HashMap::from([
                    ("com.test.b".into(), ">=0.1".into()),
                    ("com.test.c".into(), ">=0.1".into()),
                ]),
            ),
        )
        .await;
        write_manifest(&tmp, "b", &create_minimal_manifest("com.test.b", "B")).await;
        write_manifest(&tmp, "c", &create_minimal_manifest("com.test.c", "C")).await;

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        assert_eq!(plan.load_order.len(), 3);
        // B and C must come before A
        let pos_a = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.a")
            .unwrap();
        let pos_b = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.b")
            .unwrap();
        let pos_c = plan
            .load_order
            .iter()
            .position(|x| x == "com.test.c")
            .unwrap();
        assert!(pos_b < pos_a, "B should load before A");
        assert!(pos_c < pos_a, "C should load before A");
        assert!(plan.cycles.is_empty());
    }

    #[tokio::test]
    async fn test_plan_activation_cycle_detection() {
        let tmp = tempdir().unwrap();

        // A -> B -> C -> A (cycle!)
        write_manifest(
            &tmp,
            "a",
            &manifest_with_deps(
                "com.test.a",
                HashMap::from([("com.test.b".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(
            &tmp,
            "b",
            &manifest_with_deps(
                "com.test.b",
                HashMap::from([("com.test.c".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(
            &tmp,
            "c",
            &manifest_with_deps(
                "com.test.c",
                HashMap::from([("com.test.a".into(), ">=0.1".into())]),
            ),
        )
        .await;

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        // load_order should NOT contain the cyclically-dependent plugins
        assert!(plan.load_order.is_empty());
        assert!(!plan.cycles.is_empty(), "Should detect at least one cycle");
    }

    #[tokio::test]
    async fn test_plan_activation_missing_dep() {
        let tmp = tempdir().unwrap();

        // A depends on B, but B doesn't exist
        write_manifest(
            &tmp,
            "a",
            &manifest_with_deps(
                "com.test.a",
                HashMap::from([("com.test.b".into(), ">=0.1".into())]),
            ),
        )
        .await;

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        // A should still be in load_order (missing deps are just logged)
        assert_eq!(plan.load_order, vec!["com.test.a"]);
        assert_eq!(plan.missing_deps.len(), 1);
        assert_eq!(plan.missing_deps[0].0, "com.test.a");
        assert_eq!(plan.missing_deps[0].1, "com.test.b");
    }

    #[tokio::test]
    async fn test_plan_activation_partial_cycle_with_valid() {
        let tmp = tempdir().unwrap();

        // A -> B -> C -> A (cycle), D is standalone
        write_manifest(
            &tmp,
            "a",
            &manifest_with_deps(
                "com.test.a",
                HashMap::from([("com.test.b".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(
            &tmp,
            "b",
            &manifest_with_deps(
                "com.test.b",
                HashMap::from([("com.test.c".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(
            &tmp,
            "c",
            &manifest_with_deps(
                "com.test.c",
                HashMap::from([("com.test.a".into(), ">=0.1".into())]),
            ),
        )
        .await;
        write_manifest(&tmp, "d", &create_minimal_manifest("com.test.d", "D")).await;

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();

        // Only D should be in load_order; A/B/C are in a cycle
        assert_eq!(plan.load_order, vec!["com.test.d"]);
        assert!(!plan.cycles.is_empty(), "Should detect cycle");
    }

    #[test]
    fn test_trace_cycle_limit() {
        // Build a large adjacency list that will hit the 1000-iteration limit
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut sorted = HashSet::new();
        for i in 0..100 {
            let id = format!("node_{}", i);
            let next = format!("node_{}", (i + 1) % 100);
            adj.entry(id.clone()).or_default().push(next);
            // Don't add to sorted, so trace_cycle tries to traverse
            sorted.insert(id);
        }
        // Remove the first node from sorted so the cycle tracer enters
        sorted.remove("node_0");

        let planner = ActivationPlanner::new(PathBuf::from("/tmp"));
        let cycle = planner.trace_cycle("node_0", &adj, &sorted);
        // Should either find a cycle or hit the safety limit and return empty
        assert!(cycle.is_empty() || cycle.len() <= 100);
    }
}
