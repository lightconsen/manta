//! Registry for discovering and resolving capabilities.
//!
//! A [`CapabilityRegistry`] holds all registered [`Capability`](super::Capability)
//! implementations and provides name-based lookup.

use crate::capability::Capability;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of named capabilities.
///
/// Capabilities are registered by name and can be looked up individually
/// or listed by category prefix.
#[derive(Default)]
pub struct CapabilityRegistry {
    capabilities: HashMap<String, Arc<dyn Capability>>,
}

impl std::fmt::Debug for CapabilityRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityRegistry")
            .field("capabilities", &self.capabilities.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl CapabilityRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            capabilities: HashMap::new(),
        }
    }

    /// Register a capability.
    ///
    /// Panics if a capability with the same name is already registered.
    pub fn register(&mut self, capability: Arc<dyn Capability>) {
        let name = capability.name().to_string();
        assert!(
            self.capabilities.insert(name.clone(), capability).is_none(),
            "Capability '{}' is already registered",
            name
        );
    }

    /// Register or replace a capability.
    pub fn register_or_replace(&mut self, capability: Arc<dyn Capability>) {
        let name = capability.name().to_string();
        self.capabilities.insert(name, capability);
    }

    /// Look up a capability by exact name.
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn Capability>> {
        self.capabilities.get(name).cloned()
    }

    /// List all registered capability names.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.capabilities.keys().cloned().collect();
        names.sort();
        names
    }

    /// List capability names starting with `prefix`.
    pub fn list_by_prefix(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .capabilities
            .keys()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Number of registered capabilities.
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// Returns `true` if no capabilities are registered.
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Iterate over all registered capabilities.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Arc<dyn Capability>)> {
        self.capabilities.iter().map(|(k, v)| (k.as_str(), v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, CapabilityResult};
    use async_trait::async_trait;
    use serde_json::Value;

    struct TestCap {
        name: String,
    }

    #[async_trait]
    impl Capability for TestCap {
        fn name(&self) -> &str {
            &self.name
        }
        fn param_schema(&self) -> Value {
            Value::Null
        }
        async fn execute(&self, _params: Value) -> CapabilityResult {
            CapabilityResult {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }
        }
    }

    #[test]
    fn test_empty_registry() {
        let reg = CapabilityRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.resolve("nonexistent").is_none());
    }

    #[test]
    fn test_register_and_resolve() {
        let mut reg = CapabilityRegistry::new();
        reg.register(Arc::new(TestCap {
            name: "test.cap".into(),
        }));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());

        let cap = reg.resolve("test.cap");
        assert!(cap.is_some());
        assert_eq!(cap.unwrap().name(), "test.cap");
    }

    #[test]
    fn test_list_by_prefix() {
        let mut reg = CapabilityRegistry::new();
        reg.register(Arc::new(TestCap {
            name: "motor.move_to".into(),
        }));
        reg.register(Arc::new(TestCap {
            name: "motor.home".into(),
        }));
        reg.register(Arc::new(TestCap {
            name: "camera.capture".into(),
        }));

        let motor = reg.list_by_prefix("motor.");
        assert_eq!(motor.len(), 2);
        assert!(motor.contains(&"motor.home".into()));
        assert!(motor.contains(&"motor.move_to".into()));

        let camera = reg.list_by_prefix("camera.");
        assert_eq!(camera.len(), 1);
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn test_duplicate_registration_panics() {
        let mut reg = CapabilityRegistry::new();
        reg.register(Arc::new(TestCap {
            name: "dup".into(),
        }));
        reg.register(Arc::new(TestCap {
            name: "dup".into(),
        }));
    }

    #[test]
    fn test_register_or_replace_does_not_panic() {
        let mut reg = CapabilityRegistry::new();
        reg.register_or_replace(Arc::new(TestCap {
            name: "dup".into(),
        }));
        reg.register_or_replace(Arc::new(TestCap {
            name: "dup".into(),
        }));
        assert_eq!(reg.len(), 1);
    }
}
