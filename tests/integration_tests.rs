//! Integration tests for Manta
//!
//! These tests verify that the application components work together correctly.

use manta::core::models::{CreateEntityRequest, Entity, Status, UpdateEntityRequest};
use manta::core::Engine;

#[tokio::test]
async fn test_full_entity_lifecycle() {
    // Create engine
    let engine = Engine::new();

    // Create entity
    let request = CreateEntityRequest {
        name: "Integration Test Entity".to_string(),
        description: Some("Created during integration test".to_string()),
        tags: Some(vec!["test".to_string(), "integration".to_string()]),
    };

    let entity = engine.create_entity(request).unwrap();
    assert_eq!(entity.name, "Integration Test Entity");
    assert_eq!(entity.status, Status::Pending);

    let id = entity.id;

    // Retrieve entity
    let retrieved = engine.get_entity(id).unwrap();
    assert_eq!(retrieved.id, id);
    assert_eq!(retrieved.name, entity.name);

    // Update entity
    let update = UpdateEntityRequest {
        name: Some("Updated Name".to_string()),
        status: Some(Status::Active),
        ..Default::default()
    };

    let updated = engine.update_entity(id, update).unwrap();
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.status, Status::Active);

    // List entities
    let all_entities = engine.list_entities(None).unwrap();
    assert_eq!(all_entities.len(), 1);

    let active_entities = engine.list_entities(Some(Status::Active)).unwrap();
    assert_eq!(active_entities.len(), 1);

    let pending_entities = engine.list_entities(Some(Status::Pending)).unwrap();
    assert_eq!(pending_entities.len(), 0);

    // Delete entity
    engine.delete_entity(id).unwrap();

    // Verify deletion
    assert!(engine.get_entity(id).is_err());
}

#[test]
fn test_entity_validation() {
    // Empty name should fail
    let invalid = CreateEntityRequest {
        name: "".to_string(),
        description: None,
        tags: None,
    };

    assert!(invalid.validate().is_err());

    // Valid name should succeed
    let valid = CreateEntityRequest {
        name: "Valid Name".to_string(),
        description: None,
        tags: None,
    };

    assert!(valid.validate().is_ok());
}

#[test]
fn test_entity_status_transitions() {
    let engine = Engine::new();

    // Create entity
    let entity = engine
        .create_entity(CreateEntityRequest {
            name: "Status Test".to_string(),
            description: None,
            tags: None,
        })
        .unwrap();

    assert!(!entity.is_terminal());

    // Transition to active
    engine
        .update_entity(
            entity.id,
            UpdateEntityRequest {
                status: Some(Status::Active),
                ..Default::default()
            },
        )
        .unwrap();

    let active = engine.get_entity(entity.id).unwrap();
    assert!(active.is_active());
    assert!(!active.is_terminal());

    // Transition to completed (terminal state)
    engine
        .update_entity(
            entity.id,
            UpdateEntityRequest {
                status: Some(Status::Completed),
                ..Default::default()
            },
        )
        .unwrap();

    let completed = engine.get_entity(entity.id).unwrap();
    assert!(completed.is_terminal());
    assert!(!completed.is_active());
}

#[test]
fn test_concurrent_entity_creation() {
    use std::sync::Arc;
    use std::thread;

    let engine = Arc::new(Engine::new());
    let mut handles = vec![];

    // Spawn multiple threads creating entities
    for i in 0..10 {
        let engine_clone = Arc::clone(&engine);
        let handle = thread::spawn(move || {
            let request = CreateEntityRequest {
                name: format!("Thread Entity {}", i),
                description: None,
                tags: None,
            };
            engine_clone.create_entity(request).unwrap()
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    let mut ids = vec![];
    for handle in handles {
        let entity = handle.join().unwrap();
        ids.push(entity.id);
    }

    // Verify all entities were created
    assert_eq!(engine.entity_count().unwrap(), 10);

    // Verify all IDs are unique
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 10);
}

#[test]
fn test_engine_capacity_limits() {
    use manta::core::engine::EngineConfig;

    let config = EngineConfig {
        max_entities: 5,
        allow_duplicate_names: true,
    };

    let engine = Engine::with_config(config);

    // Create up to capacity
    for i in 0..5 {
        engine
            .create_entity(CreateEntityRequest {
                name: format!("Entity {}", i),
                description: None,
                tags: None,
            })
            .unwrap();
    }

    // Next creation should fail
    let result = engine.create_entity(CreateEntityRequest {
        name: "Overflow Entity".to_string(),
        description: None,
        tags: None,
    });

    assert!(result.is_err());
}

#[tokio::test]
async fn test_storage_adapter() {
    use manta::adapters::storage::{InMemoryStorage, Storage};
    use manta::core::models::Entity;

    let storage = InMemoryStorage::new();

    // Test empty storage
    assert_eq!(storage.count().await.unwrap(), 0);
    assert!(storage.health_check().await.is_ok());

    // Create entity
    let entity = Entity::new("Storage Test");
    storage.create(&entity).await.unwrap();
    assert_eq!(storage.count().await.unwrap(), 1);

    // Get entity
    let retrieved = storage.get(entity.id).await.unwrap();
    assert_eq!(retrieved.id, entity.id);

    // Update entity
    let mut updated = entity.clone();
    updated.set_name("Updated");
    storage.update(&updated).await.unwrap();

    let retrieved = storage.get(entity.id).await.unwrap();
    assert_eq!(retrieved.name, "Updated");

    // List entities
    let list = storage.list().await.unwrap();
    assert_eq!(list.len(), 1);

    // Delete entity
    storage.delete(entity.id).await.unwrap();
    assert_eq!(storage.count().await.unwrap(), 0);
    assert!(storage.get(entity.id).await.is_err());
}
