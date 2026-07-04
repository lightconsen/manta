//! Skill dependency resolution
//!
//! Provides dependency graph construction, topological ordering,
//! circular dependency detection, and version constraint checking.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::skills::semver::{Version, VersionReq};

/// A parsed dependency specification: `name: ^1.0.0`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    pub name: String,
    pub version_req: VersionReq,
}

impl DependencySpec {
    /// Parse a dependency string like `skill-name` or `skill-name: ^1.0.0`
    pub fn parse(s: &str) -> crate::Result<Self> {
        let trimmed = s.trim();

        // Support formats: "skill-name", "skill-name: ^1.0.0", "skill-name:>=1.0.0"
        let (name, req_str) = if let Some(colon_pos) = trimmed.find(':') {
            let name = trimmed[..colon_pos].trim().to_string();
            let req = trimmed[colon_pos + 1..].trim();
            (name, req)
        } else {
            (trimmed.to_string(), "*")
        };

        let version_req = VersionReq::parse(req_str)?;

        Ok(Self { name, version_req })
    }

    /// Check if an available version satisfies this dependency
    pub fn is_satisfied_by(&self, version: &Version) -> bool {
        self.version_req.matches(version)
    }
}

/// Node in the dependency graph
#[derive(Debug, Clone)]
pub struct DependencyNode {
    pub name: String,
    pub version: Version,
    pub dependencies: Vec<DependencySpec>,
    pub provides: Vec<String>,
}

/// A dependency graph for skills
#[derive(Debug, Default)]
pub struct DependencyGraph {
    nodes: HashMap<String, DependencyNode>,
}

impl DependencyGraph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self { nodes: HashMap::new() }
    }

    /// Add a node to the graph
    pub fn add_node(&mut self, node: DependencyNode) {
        self.nodes.insert(node.name.clone(), node);
    }

    /// Get a node by name
    pub fn get(&self, name: &str) -> Option<&DependencyNode> {
        self.nodes.get(name)
    }

    /// Check if a node exists
    pub fn has(&self, name: &str) -> bool {
        self.nodes.contains_key(name)
    }

    /// Get all node names
    pub fn names(&self) -> Vec<&str> {
        self.nodes.keys().map(|s| s.as_str()).collect()
    }

    /// Resolve dependencies in topological order
    /// Returns the ordered list of skill names to load/activate
    pub fn resolve(&self, root: &str) -> Result<Vec<String>, DependencyError> {
        let mut visited = HashSet::new();
        let mut temp_mark = HashSet::new();
        let mut order = Vec::new();

        fn visit(
            graph: &DependencyGraph,
            name: &str,
            visited: &mut HashSet<String>,
            temp_mark: &mut HashSet<String>,
            order: &mut Vec<String>,
            path: &mut Vec<String>,
        ) -> Result<(), DependencyError> {
            if temp_mark.contains(name) {
                return Err(DependencyError::CircularDependency { cycle: path.clone() });
            }

            if visited.contains(name) {
                return Ok(());
            }

            temp_mark.insert(name.to_string());
            path.push(name.to_string());

            if let Some(node) = graph.get(name) {
                for dep in &node.dependencies {
                    if !visited.contains(&dep.name) {
                        visit(graph, &dep.name, visited, temp_mark, order, path)?;
                    }
                }
            }

            path.pop();
            temp_mark.remove(name);
            visited.insert(name.to_string());
            order.push(name.to_string());

            Ok(())
        }

        let mut path = Vec::new();
        visit(self, root, &mut visited, &mut temp_mark, &mut order, &mut path)?;

        Ok(order)
    }

    /// Resolve all skills in the graph, ensuring all dependencies are met
    pub fn resolve_all(&self) -> Result<Vec<String>, DependencyError> {
        // Build reverse adjacency list: for each node, which nodes depend on it
        let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();
        let mut in_degree: HashMap<String, usize> = HashMap::new();

        // Initialize in-degree = number of dependencies each node has
        for (name, node) in &self.nodes {
            in_degree.insert(name.clone(), node.dependencies.len());
            for dep in &node.dependencies {
                if self.has(&dep.name) {
                    reverse_deps
                        .entry(dep.name.clone())
                        .or_default()
                        .push(name.clone());
                }
            }
        }

        // Kahn's algorithm: start with nodes that have 0 dependencies
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(name, _)| name.clone())
            .collect();

        let mut order = Vec::new();

        while let Some(name) = queue.pop_front() {
            order.push(name.clone());

            // Decrease in-degree of nodes that depend on this one
            if let Some(dependents) = reverse_deps.get(&name) {
                for dependent in dependents {
                    if let Some(deg) = in_degree.get_mut(dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        if order.len() != self.nodes.len() {
            return Err(DependencyError::CircularDependency { cycle: Vec::new() });
        }

        Ok(order)
    }

    /// Check version constraints for all dependencies
    pub fn check_versions(&self) -> Result<(), DependencyError> {
        for node in self.nodes.values() {
            for dep in &node.dependencies {
                if let Some(dep_node) = self.get(&dep.name) {
                    if !dep.is_satisfied_by(&dep_node.version) {
                        return Err(DependencyError::VersionMismatch {
                            skill: node.name.clone(),
                            dependency: dep.name.clone(),
                            required: dep.version_req.to_string(),
                            found: dep_node.version.to_string(),
                        });
                    }
                } else {
                    return Err(DependencyError::MissingDependency {
                        skill: node.name.clone(),
                        dependency: dep.name.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Find which skill provides a given capability
    pub fn find_provider(&self, capability: &str) -> Option<&str> {
        for node in self.nodes.values() {
            if node.provides.iter().any(|p| p == capability) {
                return Some(&node.name);
            }
        }
        None
    }

    /// Build a chain of skills: root and its dependencies in order.
    ///
    /// Note: Skill-level chain annotations (e.g., `chain: ["summarize"]`)
    /// are handled by [`SkillManager::build_execution_chain`], not here.
    /// This method returns the topological order for the dependency graph
    /// rooted at `root`.
    pub fn build_chain(&self, root: &str) -> Result<Vec<String>, DependencyError> {
        self.resolve(root)
    }
}

/// Dependency resolution errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    /// A dependency is missing
    MissingDependency { skill: String, dependency: String },
    /// Version constraint not satisfied
    VersionMismatch {
        skill: String,
        dependency: String,
        required: String,
        found: String,
    },
    /// Circular dependency detected
    CircularDependency { cycle: Vec<String> },
}

impl std::fmt::Display for DependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyError::MissingDependency { skill, dependency } => {
                write!(f, "Skill '{}' requires '{}' which is not available", skill, dependency)
            }
            DependencyError::VersionMismatch {
                skill,
                dependency,
                required,
                found,
            } => {
                write!(
                    f,
                    "Skill '{}' requires {} ({}) but found {} ({})",
                    skill, dependency, required, dependency, found
                )
            }
            DependencyError::CircularDependency { cycle } => {
                if cycle.is_empty() {
                    write!(f, "Circular dependency detected")
                } else {
                    write!(f, "Circular dependency: {}", cycle.join(" -> "))
                }
            }
        }
    }
}

impl std::error::Error for DependencyError {}

/// Resolve a skill and all its dependencies
pub fn resolve_skill_chain(
    skills: &HashMap<String, (Version, Vec<DependencySpec>, Vec<String>)>,
    root: &str,
) -> Result<Vec<String>, DependencyError> {
    let mut graph = DependencyGraph::new();

    for (name, (version, deps, provides)) in skills {
        graph.add_node(DependencyNode {
            name: name.clone(),
            version: *version,
            dependencies: deps.clone(),
            provides: provides.clone(),
        });
    }

    // Check versions first
    graph.check_versions()?;

    // Then resolve in topological order
    graph.resolve(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(name: &str, version: &str, deps: Vec<&str>) -> DependencyNode {
        DependencyNode {
            name: name.to_string(),
            version: Version::parse(version).unwrap(),
            dependencies: deps
                .into_iter()
                .map(|d| DependencySpec::parse(d).unwrap())
                .collect(),
            provides: Vec::new(),
        }
    }

    #[test]
    fn test_dependency_spec_parse() {
        let spec = DependencySpec::parse("weather").unwrap();
        assert_eq!(spec.name, "weather");
        assert!(spec.version_req.matches(&Version::new(99, 0, 0)));

        let spec = DependencySpec::parse("weather: ^1.0.0").unwrap();
        assert_eq!(spec.name, "weather");
        assert!(spec.is_satisfied_by(&Version::new(1, 2, 0)));
        assert!(!spec.is_satisfied_by(&Version::new(2, 0, 0)));
    }

    #[test]
    fn test_graph_resolve_linear() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("a", "1.0.0", vec!["b: >=1.0.0"]));
        graph.add_node(make_node("b", "1.0.0", vec!["c: >=1.0.0"]));
        graph.add_node(make_node("c", "1.0.0", vec![]));

        let order = graph.resolve("a").unwrap();
        assert_eq!(order, vec!["c", "b", "a"]);
    }

    #[test]
    fn test_graph_resolve_diamond() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("app", "1.0.0", vec!["lib1: >=1.0.0", "lib2: >=1.0.0"]));
        graph.add_node(make_node("lib1", "1.0.0", vec!["base: >=1.0.0"]));
        graph.add_node(make_node("lib2", "1.0.0", vec!["base: >=1.0.0"]));
        graph.add_node(make_node("base", "1.0.0", vec![]));

        let order = graph.resolve("app").unwrap();
        assert_eq!(order[0], "base"); // base must be first
        assert_eq!(order.last().unwrap(), "app"); // app must be last
    }

    #[test]
    fn test_graph_circular() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("a", "1.0.0", vec!["b: >=1.0.0"]));
        graph.add_node(make_node("b", "1.0.0", vec!["a: >=1.0.0"]));

        let err = graph.resolve("a").unwrap_err();
        assert!(matches!(err, DependencyError::CircularDependency { .. }));
    }

    #[test]
    fn test_version_mismatch() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("app", "1.0.0", vec!["lib: ^2.0.0"]));
        graph.add_node(make_node("lib", "1.5.0", vec![]));

        let err = graph.check_versions().unwrap_err();
        assert!(matches!(err, DependencyError::VersionMismatch { .. }));
    }

    #[test]
    fn test_missing_dependency() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("app", "1.0.0", vec!["missing: >=1.0.0"]));

        let err = graph.check_versions().unwrap_err();
        assert!(matches!(err, DependencyError::MissingDependency { .. }));
    }

    #[test]
    fn test_graph_resolve_all() {
        let mut graph = DependencyGraph::new();
        graph.add_node(make_node("a", "1.0.0", vec![]));
        graph.add_node(make_node("b", "1.0.0", vec!["a: >=1.0.0"]));
        graph.add_node(make_node("c", "1.0.0", vec!["a: >=1.0.0"]));

        let order = graph.resolve_all().unwrap();
        assert_eq!(order[0], "a");
        assert!(order.len() == 3);
    }

    #[test]
    fn test_find_provider() {
        let mut graph = DependencyGraph::new();
        graph.add_node(DependencyNode {
            name: "weather".to_string(),
            version: Version::new(1, 0, 0),
            dependencies: vec![],
            provides: vec!["forecast".to_string(), "alerts".to_string()],
        });

        assert_eq!(graph.find_provider("forecast"), Some("weather"));
        assert_eq!(graph.find_provider("alerts"), Some("weather"));
        assert_eq!(graph.find_provider("missing"), None);
    }

    #[test]
    fn test_resolve_skill_chain_helper() {
        let mut skills = HashMap::new();
        skills.insert(
            "app".to_string(),
            (
                Version::new(1, 0, 0),
                vec![DependencySpec::parse("lib: >=1.0.0").unwrap()],
                vec![],
            ),
        );
        skills.insert(
            "lib".to_string(),
            (Version::new(1, 0, 0), vec![], vec!["base-feature".to_string()]),
        );

        let chain = resolve_skill_chain(&skills, "app").unwrap();
        assert_eq!(chain, vec!["lib", "app"]);
    }
}
