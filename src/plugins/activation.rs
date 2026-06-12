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
        self.triggers
            .as_ref()
            .map(|t| t.clone())
            .unwrap_or_default()
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

        // Group by trigger type
        let mut lazy_plugins: HashMap<String, Vec<String>> = HashMap::new();
        for (id, manifest, _path) in &manifests {
            for trigger in manifest.get_triggers() {
                let key = trigger_key(&trigger);
                lazy_plugins.entry(key).or_default().push(id.clone());
            }
        }

        Ok(ActivationPlan {
            load_order,
            lazy_plugins,
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
        let plan = self.plan_activation().await?;

        let mut startup = Vec::new();
        let lazy_set: HashSet<String> = plan.lazy_plugins.values().flatten().cloned().collect();

        for id in &plan.load_order {
            if !lazy_set.contains(id) {
                startup.push(id.clone());
            }
        }

        // Also include plugins with OnStartup trigger that are in load_order
        // (they won't be in lazy_set if only OnStartup is defined)
        let manifests = self.discover_manifests().await?;
        for (id, manifest, _path) in &manifests {
            if manifest.has_startup_trigger() && plan.load_order.contains(id) {
                if !startup.contains(id) {
                    startup.push(id.clone());
                }
            }
        }

        Ok(startup)
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
    /// Lazy-activated plugins by trigger key.
    pub lazy_plugins: HashMap<String, Vec<String>>,
    /// Cyclic dependencies detected.
    pub cycles: Vec<Vec<String>>,
    /// Missing dependencies: (plugin_id, dep_name, constraint).
    pub missing_deps: Vec<(String, String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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

    #[tokio::test]
    async fn test_plan_activation_empty_dir() {
        let tmp = tempdir().unwrap();
        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let plan = planner.plan_activation().await.unwrap();
        assert!(plan.load_order.is_empty());
        assert!(plan.lazy_plugins.is_empty());
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
    async fn test_trigger_key() {
        assert_eq!(trigger_key(&PluginTrigger::Command("deploy".to_string())), "command:deploy");
        assert_eq!(trigger_key(&PluginTrigger::Provider("openai".to_string())), "provider:openai");
        assert_eq!(trigger_key(&PluginTrigger::Channel("slack".to_string())), "channel:slack");
        assert_eq!(trigger_key(&PluginTrigger::OnStartup), "on_startup");
        assert_eq!(
            trigger_key(&PluginTrigger::Capability("tools".to_string())),
            "capability:tools"
        );
    }

    #[tokio::test]
    async fn test_triggered_plugins() {
        let tmp = tempdir().unwrap();

        // Plugin with command trigger
        let p1_dir = tmp.path().join("p1");
        tokio::fs::create_dir_all(&p1_dir).await.unwrap();
        tokio::fs::write(
            p1_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest_with_triggers(
                "com.test.p1",
                vec![PluginTrigger::Command("deploy".to_string())],
            ))
            .unwrap(),
        )
        .await
        .unwrap();

        // Plugin with startup trigger
        let p2_dir = tmp.path().join("p2");
        tokio::fs::create_dir_all(&p2_dir).await.unwrap();
        tokio::fs::write(
            p2_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest_with_triggers(
                "com.test.p2",
                vec![PluginTrigger::OnStartup],
            ))
            .unwrap(),
        )
        .await
        .unwrap();

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());

        let triggered = planner
            .triggered_plugins(&PluginTrigger::Command("deploy".to_string()))
            .await
            .unwrap();
        assert_eq!(triggered, vec!["com.test.p1"]);

        let no_match = planner
            .triggered_plugins(&PluginTrigger::Provider("nonexistent".to_string()))
            .await
            .unwrap();
        assert!(no_match.is_empty());
    }

    #[tokio::test]
    async fn test_startup_plugins() {
        let tmp = tempdir().unwrap();

        // Plugin with startup trigger
        let p1_dir = tmp.path().join("startup-plugin");
        tokio::fs::create_dir_all(&p1_dir).await.unwrap();
        tokio::fs::write(
            p1_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest_with_triggers(
                "com.test.startup",
                vec![PluginTrigger::OnStartup],
            ))
            .unwrap(),
        )
        .await
        .unwrap();

        // Plugin without trigger (should also be startup)
        let p2_dir = tmp.path().join("no-trigger");
        tokio::fs::create_dir_all(&p2_dir).await.unwrap();
        tokio::fs::write(
            p2_dir.join("plugin.json"),
            serde_json::to_string_pretty(&create_minimal_manifest(
                "com.test.notrigger",
                "NoTrigger",
            ))
            .unwrap(),
        )
        .await
        .unwrap();

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        let startup = planner.startup_plugins().await.unwrap();

        // Both plugins should be in the startup list
        assert!(startup.contains(&"com.test.startup".to_string()));
        assert!(startup.contains(&"com.test.notrigger".to_string()));
    }

    #[test]
    fn test_get_triggers_none() {
        let manifest = PluginManifest::minimal("test", "Test");
        assert!(manifest.get_triggers().is_empty());
    }

    #[test]
    fn test_has_startup_trigger() {
        let manifest = PluginManifest::minimal("test", "Test");
        assert!(!manifest.has_startup_trigger());

        let manifest_with = PluginManifest {
            triggers: Some(vec![PluginTrigger::OnStartup]),
            ..PluginManifest::minimal("test", "Test")
        };
        assert!(manifest_with.has_startup_trigger());
    }

    #[test]
    fn test_dependency_ids_empty() {
        let manifest = PluginManifest::minimal("test", "Test");
        assert!(manifest.get_dependency_ids().is_empty());
    }

    #[test]
    fn test_serde_roundtrip() {
        let trigger = PluginTrigger::Command("deploy".to_string());
        let json = serde_json::to_value(&trigger).unwrap();
        let decoded: PluginTrigger = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, PluginTrigger::Command(ref n) if n == "deploy"));
    }

    #[tokio::test]
    async fn test_activation_plan_serde_triggers() {
        let tmp = tempdir().unwrap();
        let p_dir = tmp.path().join("plugin");
        tokio::fs::create_dir_all(&p_dir).await.unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.triggers",
            "name": "Triggers",
            "version": "0.1.0",
            "description": "Test",
            "triggers": [
                {"command": "deploy"},
                {"on_startup": null}
            ]
        });
        tokio::fs::write(
            p_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let planner = ActivationPlanner::new(tmp.path().to_path_buf());
        // Should not crash — just validates that serialization works
        let _plan = planner.plan_activation().await.unwrap();
    }
}
