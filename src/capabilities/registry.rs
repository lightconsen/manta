//! Capability set registry — manages platform-specific tool collections.

use super::{CapabilitySet, OsControlScope, ToolConflictStrategy};
use crate::tools::ToolRegistry;
use std::collections::{HashMap, HashSet};

/// Registry of all capability sets, with runtime environment detection.
pub struct CapabilityRegistry {
    sets: Vec<Box<dyn CapabilitySet>>,
    disabled: HashSet<String>,
    availability_cache: std::sync::RwLock<HashMap<String, bool>>,
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            sets: Vec::new(),
            disabled: HashSet::new(),
            availability_cache: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Register a capability set.
    pub fn register(&mut self, set: Box<dyn CapabilitySet>) {
        self.sets.push(set);
        self.refresh_cache();
    }

    /// Disable a set by ID.
    pub fn disable(&mut self, set_id: &str) {
        self.disabled.insert(set_id.to_string());
    }

    /// Re-enable a previously disabled set.
    pub fn enable(&mut self, set_id: &str) {
        self.disabled.remove(set_id);
    }

    /// Whether a set is explicitly disabled.
    pub fn is_disabled(&self, set_id: &str) -> bool {
        self.disabled.contains(set_id)
    }

    /// All registered sets (regardless of availability).
    pub fn all_sets(&self) -> Vec<&dyn CapabilitySet> {
        self.sets.iter().map(|s| s.as_ref()).collect()
    }

    /// Sets that pass environment checks and are not disabled.
    pub fn available_sets(&self) -> Vec<&dyn CapabilitySet> {
        self.sets
            .iter()
            .filter(|s| !self.disabled.contains(s.id()))
            .filter(|s| self.check_availability(s.id(), s.as_ref()))
            .map(|s| s.as_ref())
            .collect()
    }

    /// Export tools from all available sets into a `ToolRegistry`.
    pub fn export_to_tool_registry(
        &self,
        registry: &mut ToolRegistry,
        strategy: ToolConflictStrategy,
    ) {
        let mut seen = HashSet::new();

        for set in self.available_sets() {
            for tool in set.tools() {
                let name = tool.name().to_string();
                if seen.contains(&name) {
                    match strategy {
                        ToolConflictStrategy::Reject => {
                            panic!(
                                "Tool '{}' conflicts between capability sets",
                                name
                            );
                        }
                        ToolConflictStrategy::Override => {
                            registry.register(tool);
                        }
                    }
                } else {
                    seen.insert(name.clone());
                    registry.register(tool);
                }
            }
        }
    }

    /// Export only sets within a max permission scope.
    pub fn export_with_scope(
        &self,
        registry: &mut ToolRegistry,
        max_scope: OsControlScope,
        strategy: ToolConflictStrategy,
    ) {
        let mut seen = HashSet::new();

        for set in self.available_sets() {
            if set.scope() > max_scope {
                continue;
            }
            for tool in set.tools() {
                let name = tool.name().to_string();
                if seen.contains(&name) {
                    match strategy {
                        ToolConflictStrategy::Reject => {
                            panic!(
                                "Tool '{}' conflicts between capability sets",
                                name
                            );
                        }
                        ToolConflictStrategy::Override => {
                            registry.register(tool);
                        }
                    }
                } else {
                    seen.insert(name.clone());
                    registry.register(tool);
                }
            }
        }
    }

    /// Clear availability cache.
    pub fn refresh_cache(&self) {
        if let Ok(mut cache) = self.availability_cache.write() {
            cache.clear();
        }
    }

    fn check_availability(&self, id: &str, set: &dyn CapabilitySet) -> bool {
        if let Ok(cache) = self.availability_cache.read() {
            if let Some(cached) = cache.get(id) {
                return *cached;
            }
        }
        let available = set.is_available();
        if let Ok(mut cache) = self.availability_cache.write() {
            cache.insert(id.to_string(), available);
        }
        available
    }
}
