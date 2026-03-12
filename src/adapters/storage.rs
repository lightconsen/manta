//! Storage adapter for Manta
//!
//! This module provides storage abstractions and implementations.

use crate::core::models::{Entity, Id};
use crate::error::MantaError;
use async_trait::async_trait;
use sqlx::Row;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// Errors that can occur during storage operations
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Entity not found: {0}")]
    NotFound(Id),

    #[error("Storage is full")]
    Full,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Storage backend error: {0}")]
    Backend(String),
}

impl From<StorageError> for MantaError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound(id) => MantaError::NotFound {
                resource: format!("Entity {} not found", id),
            },
            StorageError::Full => MantaError::Validation("Storage is full".to_string()),
            StorageError::Io(e) => MantaError::Io(e),
            StorageError::Serialization(msg) => MantaError::Internal(format!("Serialization error: {}", msg)),
            StorageError::Backend(msg) => MantaError::Internal(format!("Storage backend: {}", msg)),
        }
    }
}

/// Storage trait for entity persistence
#[async_trait]
pub trait Storage: Send + Sync {
    /// Get an entity by ID
    async fn get(&self, id: Id) -> Result<Entity, StorageError>;

    /// List all entities, optionally filtered by status
    async fn list(&self) -> Result<Vec<Entity>, StorageError>;

    /// Create a new entity
    async fn create(&self, entity: &Entity) -> Result<(), StorageError>;

    /// Update an existing entity
    async fn update(&self, entity: &Entity) -> Result<(), StorageError>;

    /// Delete an entity
    async fn delete(&self, id: Id) -> Result<(), StorageError>;

    /// Count total entities
    async fn count(&self) -> Result<usize, StorageError>;

    /// Check if storage is healthy
    async fn health_check(&self) -> Result<(), StorageError>;
}

/// In-memory storage implementation
#[derive(Debug, Clone)]
pub struct InMemoryStorage {
    data: Arc<RwLock<HashMap<Id, Entity>>>,
    max_size: usize,
}

impl InMemoryStorage {
    /// Create a new in-memory storage with default capacity
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// Create a new in-memory storage with specified max size
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::with_capacity(max_size.min(1000)))),
            max_size,
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Storage for InMemoryStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        data.get(&id)
            .cloned()
            .ok_or(StorageError::NotFound(id))
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        Ok(data.values().cloned().collect())
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        if data.len() >= self.max_size {
            return Err(StorageError::Full);
        }

        data.insert(entity.id, entity.clone());
        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        if !data.contains_key(&entity.id) {
            return Err(StorageError::NotFound(entity.id));
        }

        data.insert(entity.id, entity.clone());
        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        data.remove(&id)
            .ok_or(StorageError::NotFound(id))
            .map(|_| ())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        Ok(data.len())
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // In-memory storage is always healthy unless we can't acquire the lock
        let _guard = self.data.read().map_err(|_| {
            StorageError::Backend("Storage lock poisoned".to_string())
        })?;
        drop(_guard);
        Ok(())
    }
}

/// File-based storage implementation
#[derive(Debug, Clone)]
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new file storage at the given path
    pub fn new(base_path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let base_path = base_path.into();
        std::fs::create_dir_all(&base_path)?;

        Ok(Self { base_path })
    }

    /// Get the path for a specific entity
    fn entity_path(&self, id: Id) -> PathBuf {
        self.base_path.join(format!("{}.json", id))
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let path = self.entity_path(id);

        if !path.exists() {
            return Err(StorageError::NotFound(id));
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let entity: Entity = serde_json::from_str(&content)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        Ok(entity)
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let mut entities = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let content = tokio::fs::read_to_string(&path).await?;
                if let Ok(entity) = serde_json::from_str::<Entity>(&content) {
                    entities.push(entity);
                }
            }
        }

        Ok(entities)
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let path = self.entity_path(entity.id);

        if path.exists() {
            return Err(StorageError::Backend(
                format!("Entity {} already exists", entity.id)
            ));
        }

        let content = serde_json::to_string_pretty(entity)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let path = self.entity_path(entity.id);

        if !path.exists() {
            return Err(StorageError::NotFound(entity.id));
        }

        let content = serde_json::to_string_pretty(entity)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let path = self.entity_path(id);

        if !path.exists() {
            return Err(StorageError::NotFound(id));
        }

        tokio::fs::remove_file(&path).await?;
        Ok(())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let mut count = 0;
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                count += 1;
            }
        }

        Ok(count)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // Check if we can read the directory
        let _ = tokio::fs::read_dir(&self.base_path).await?;
        Ok(())
    }
}

/// SQLite-backed storage implementation
#[derive(Debug, Clone)]
pub struct SqliteStorage {
    pool: sqlx::SqlitePool,
}

impl SqliteStorage {
    /// Create a new SQLite storage with an existing pool
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new SQLite storage from a database URL
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let pool = sqlx::SqlitePool::connect(database_url)
            .await
            .map_err(|e| StorageError::Backend(format!("Failed to connect: {}", e)))?;

        let storage = Self { pool };
        storage.init().await?;
        Ok(storage)
    }

    /// Initialize the database schema
    async fn init(&self) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                tags TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                version INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create table: {}", e)))?;

        // Create index on status for faster filtering
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_entities_status ON entities(status)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create index: {}", e)))?;

        Ok(())
    }

    /// Convert a database row to an Entity
    fn row_to_entity(row: &sqlx::sqlite::SqliteRow) -> Result<Entity, StorageError> {
        use chrono::DateTime;
        use crate::core::models::{Metadata, Status};

        let id_str: String = row.get("id");
        let id = Id::parse(&id_str)
            .map_err(|_| StorageError::Serialization(format!("Invalid ID: {}", id_str)))?;

        let name: String = row.get("name");
        let description: Option<String> = row.get("description");

        let tags_str: String = row.get("tags");
        let tags: Vec<String> = serde_json::from_str(&tags_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let status_str: String = row.get("status");
        let status = status_str.parse::<Status>()
            .map_err(|e| StorageError::Serialization(format!("Invalid status: {} - {}", status_str, e)))?;

        let created_at_str: String = row.get("created_at");
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?
            .with_timezone(&chrono::Utc);

        let updated_at_str: String = row.get("updated_at");
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?
            .with_timezone(&chrono::Utc);

        let version: i64 = row.get("version");

        Ok(Entity {
            id,
            name,
            description,
            tags: Some(tags),
            status,
            metadata: Metadata {
                created_at,
                updated_at,
                version: version as u64,
                tags: None,
            },
        })
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM entities WHERE id = ?1"
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        match row {
            Some(row) => Self::row_to_entity(&row),
            None => Err(StorageError::NotFound(id)),
        }
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM entities ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        rows.iter()
            .map(Self::row_to_entity)
            .collect()
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&entity.tags)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        sqlx::query(
            r#"
            INSERT INTO entities (id, name, description, tags, status, created_at, updated_at, version)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#
        )
        .bind(entity.id.to_string())
        .bind(&entity.name)
        .bind(&entity.description)
        .bind(tags_json)
        .bind(entity.status.to_string())
        .bind(entity.metadata.created_at.to_rfc3339())
        .bind(entity.metadata.updated_at.to_rfc3339())
        .bind(entity.metadata.version as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&entity.tags)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let result = sqlx::query(
            r#"
            UPDATE entities
            SET name = ?1, description = ?2, tags = ?3, status = ?4,
                updated_at = ?5, version = ?6
            WHERE id = ?7
            "#
        )
        .bind(&entity.name)
        .bind(&entity.description)
        .bind(tags_json)
        .bind(entity.status.to_string())
        .bind(entity.metadata.updated_at.to_rfc3339())
        .bind(entity.metadata.version as i64)
        .bind(entity.id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(entity.id));
        }

        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM entities WHERE id = ?1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(id));
        }

        Ok(())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM entities")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        let count: i64 = row.get("count");
        Ok(count as usize)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // Try to execute a simple query
        let _: (i64,) = sqlx::query_as("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Entity;

    #[tokio::test]
    async fn test_in_memory_storage() {
        let storage = InMemoryStorage::new();

        // Test empty storage
        assert_eq!(storage.count().await.unwrap(), 0);

        // Create entity
        let entity = Entity::new("Test Entity");
        storage.create(&entity).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 1);

        // Get entity
        let retrieved = storage.get(entity.id).await.unwrap();
        assert_eq!(retrieved.id, entity.id);
        assert_eq!(retrieved.name, entity.name);

        // Update entity
        let mut updated = entity.clone();
        updated.set_name("Updated Name");
        storage.update(&updated).await.unwrap();

        let retrieved = storage.get(entity.id).await.unwrap();
        assert_eq!(retrieved.name, "Updated Name");

        // Delete entity
        storage.delete(entity.id).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 0);
        assert!(storage.get(entity.id).await.is_err());
    }

    #[tokio::test]
    async fn test_storage_capacity() {
        let storage = InMemoryStorage::with_capacity(2);

        storage.create(&Entity::new("Entity 1")).await.unwrap();
        storage.create(&Entity::new("Entity 2")).await.unwrap();

        // Third entity should fail
        assert!(storage.create(&Entity::new("Entity 3")).await.is_err());
    }
}
