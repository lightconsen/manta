//! Core engine for Manta
//!
//! The engine contains the main business logic and orchestrates
//! operations. It is independent of external adapters.

use super::models::{CreateEntityRequest, Entity, Id, Status, UpdateEntityRequest};
use crate::error::{MantaError, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, instrument, warn};

/// The main engine that coordinates all business logic
#[derive(Debug, Clone)]
pub struct Engine {
    /// In-memory storage for entities (would be replaced with proper storage in production)
    entities: Arc<RwLock<HashMap<Id, Entity>>>,
    /// Engine configuration
    config: EngineConfig,
}

/// Configuration for the engine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum number of entities allowed
    pub max_entities: usize,
    /// Whether to allow duplicate names
    pub allow_duplicate_names: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_entities: 10_000,
            allow_duplicate_names: true,
        }
    }
}

impl Engine {
    /// Create a new engine with default configuration
    pub fn new() -> Self {
        Self::with_config(EngineConfig::default())
    }

    /// Create a new engine with custom configuration
    pub fn with_config(config: EngineConfig) -> Self {
        info!("Initializing Manta engine");
        Self {
            entities: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new entity
    #[instrument(skip(self, request), fields(entity_name = %request.name))]
    pub fn create_entity(&self, request: CreateEntityRequest) -> Result<Entity> {
        debug!("Validating create entity request");
        request.validate()?;

        // Check entity limit
        {
            let entities = self.entities.read().map_err(|_| {
                MantaError::Internal("Failed to acquire read lock".to_string())
            })?;

            if entities.len() >= self.config.max_entities {
                return Err(MantaError::Validation(format!(
                    "Maximum number of entities ({}) reached",
                    self.config.max_entities
                )));
            }

            // Check for duplicate names if not allowed
            if !self.config.allow_duplicate_names {
                if entities.values().any(|e| e.name == request.name) {
                    return Err(MantaError::Validation(format!(
                        "Entity with name '{}' already exists",
                        request.name
                    )));
                }
            }
        }

        let entity = request.into_entity();
        let id = entity.id;

        {
            let mut entities = self.entities.write().map_err(|_| {
                MantaError::Internal("Failed to acquire write lock".to_string())
            })?;
            entities.insert(id, entity.clone());
        }

        info!(entity_id = %id, "Entity created successfully");
        Ok(entity)
    }

    /// Get an entity by ID
    #[instrument(skip(self), fields(entity_id = %id))]
    pub fn get_entity(&self, id: Id) -> Result<Entity> {
        let entities = self.entities.read().map_err(|_| {
            MantaError::Internal("Failed to acquire read lock".to_string())
        })?;

        entities
            .get(&id)
            .cloned()
            .ok_or_else(|| MantaError::NotFound { resource: format!("Entity with ID '{}' not found", id) })
    }

    /// List all entities with optional filtering
    #[instrument(skip(self))]
    pub fn list_entities(&self, filter: Option<Status>) -> Result<Vec<Entity>> {
        let entities = self.entities.read().map_err(|_| {
            MantaError::Internal("Failed to acquire read lock".to_string())
        })?;

        let mut result: Vec<Entity> = if let Some(status) = filter {
            entities
                .values()
                .filter(|e| e.status == status)
                .cloned()
                .collect()
        } else {
            entities.values().cloned().collect()
        };

        // Sort by creation date (newest first)
        result.sort_by(|a, b| {
            b.metadata
                .created_at
                .cmp(&a.metadata.created_at)
        });

        debug!(count = result.len(), "Listed entities");
        Ok(result)
    }

    /// Update an existing entity
    #[instrument(skip(self, request), fields(entity_id = %id))]
    pub fn update_entity(&self, id: Id, request: UpdateEntityRequest) -> Result<Entity> {
        let mut entities = self.entities.write().map_err(|_| {
            MantaError::Internal("Failed to acquire write lock".to_string())
        })?;

        let entity = entities
            .get_mut(&id)
            .ok_or_else(|| MantaError::NotFound { resource: format!("Entity with ID '{}' not found", id) })?;

        request.apply(entity)?;

        info!("Entity updated successfully");
        Ok(entity.clone())
    }

    /// Delete an entity
    #[instrument(skip(self), fields(entity_id = %id))]
    pub fn delete_entity(&self, id: Id) -> Result<()> {
        let mut entities = self.entities.write().map_err(|_| {
            MantaError::Internal("Failed to acquire write lock".to_string())
        })?;

        if entities.remove(&id).is_none() {
            return Err(MantaError::NotFound {
                resource: format!("Entity with ID '{}' not found", id),
            });
        }

        info!("Entity deleted successfully");
        Ok(())
    }

    /// Get entity count
    pub fn entity_count(&self) -> Result<usize> {
        let entities = self.entities.read().map_err(|_| {
            MantaError::Internal("Failed to acquire read lock".to_string())
        })?;
        Ok(entities.len())
    }

    /// Archive completed/failed entities older than a certain age
    #[instrument(skip(self))]
    pub fn archive_old_entities(&self, max_age_days: i64) -> Result<usize> {
        use chrono::Duration;

        let cutoff = chrono::Utc::now() - Duration::days(max_age_days);
        let mut archived_count = 0;

        let mut entities = self.entities.write().map_err(|_| {
            MantaError::Internal("Failed to acquire write lock".to_string())
        })?;

        for entity in entities.values_mut() {
            if entity.is_terminal() && entity.metadata.updated_at < cutoff {
                if entity.status != Status::Archived {
                    entity.set_status(Status::Archived);
                    archived_count += 1;
                }
            }
        }

        info!(count = archived_count, "Archived old entities");
        Ok(archived_count)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_entity() {
        let engine = Engine::new();

        let request = CreateEntityRequest {
            name: "Test Entity".to_string(),
            description: Some("A test".to_string()),
            tags: None,
        };

        let created = engine.create_entity(request).unwrap();
        let retrieved = engine.get_entity(created.id).unwrap();

        assert_eq!(created.id, retrieved.id);
        assert_eq!(created.name, retrieved.name);
    }

    #[test]
    fn test_get_nonexistent_entity() {
        let engine = Engine::new();
        let id = Id::new();

        assert!(engine.get_entity(id).is_err());
    }

    #[test]
    fn test_list_entities_with_filter() {
        let engine = Engine::new();

        // Create pending entity
        let pending = engine
            .create_entity(CreateEntityRequest {
                name: "Pending".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        // Create and update to completed
        let completed = engine
            .create_entity(CreateEntityRequest {
                name: "Completed".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        // Update the entity in the engine to completed
        engine
            .update_entity(
                completed.id,
                UpdateEntityRequest {
                    status: Some(Status::Completed),
                    ..Default::default()
                },
            )
            .unwrap();

        // Update the pending entity to active
        engine
            .update_entity(
                pending.id,
                UpdateEntityRequest {
                    status: Some(Status::Active),
                    ..Default::default()
                },
            )
            .unwrap();

        let active_list = engine.list_entities(Some(Status::Active)).unwrap();
        assert_eq!(active_list.len(), 1);
        assert_eq!(active_list[0].id, pending.id);

        let completed_list = engine.list_entities(Some(Status::Completed)).unwrap();
        assert_eq!(completed_list.len(), 1);
        assert_eq!(completed_list[0].id, completed.id);

        let pending_list = engine.list_entities(Some(Status::Pending)).unwrap();
        assert_eq!(pending_list.len(), 0);
    }

    #[test]
    fn test_delete_entity() {
        let engine = Engine::new();

        let entity = engine
            .create_entity(CreateEntityRequest {
                name: "To Delete".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        assert!(engine.delete_entity(entity.id).is_ok());
        assert!(engine.get_entity(entity.id).is_err());
    }

    #[test]
    fn test_entity_count() {
        let engine = Engine::new();
        assert_eq!(engine.entity_count().unwrap(), 0);

        engine
            .create_entity(CreateEntityRequest {
                name: "Entity 1".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        assert_eq!(engine.entity_count().unwrap(), 1);
    }

    #[test]
    fn test_max_entities_limit() {
        let config = EngineConfig {
            max_entities: 2,
            allow_duplicate_names: true,
        };
        let engine = Engine::with_config(config);

        engine
            .create_entity(CreateEntityRequest {
                name: "Entity 1".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        engine
            .create_entity(CreateEntityRequest {
                name: "Entity 2".to_string(),
                description: None,
                tags: None,
            })
            .unwrap();

        // Third entity should fail
        assert!(engine
            .create_entity(CreateEntityRequest {
                name: "Entity 3".to_string(),
                description: None,
                tags: None,
            })
            .is_err());
    }
}
