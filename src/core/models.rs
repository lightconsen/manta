//! Domain models for Manta
//!
//! This module defines the core data structures used throughout
//! the application. These models are framework-agnostic and
//! represent the business domain.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique identifier for entities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Id(pub Uuid);

impl Id {
    /// Create a new random ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse an ID from a string
    pub fn parse(s: &str) -> crate::Result<Self> {
        Ok(Self(Uuid::parse_str(s).map_err(|e| {
            crate::error::MantaError::Validation(format!("Invalid UUID: {}", e))
        })?))
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<Uuid> for Id {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}

/// Status of an entity or operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Entity is pending initialization
    Pending,
    /// Entity is active and operational
    Active,
    /// Entity is paused
    Paused,
    /// Entity has completed
    Completed,
    /// Entity has failed
    Failed,
    /// Entity is archived
    Archived,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Pending => write!(f, "pending"),
            Status::Active => write!(f, "active"),
            Status::Paused => write!(f, "paused"),
            Status::Completed => write!(f, "completed"),
            Status::Failed => write!(f, "failed"),
            Status::Archived => write!(f, "archived"),
        }
    }
}

impl Default for Status {
    fn default() -> Self {
        Status::Pending
    }
}

/// Metadata attached to entities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// When the entity was created
    pub created_at: DateTime<Utc>,
    /// When the entity was last updated
    pub updated_at: DateTime<Utc>,
    /// Version for optimistic locking
    pub version: u64,
    /// Optional tags
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

impl Metadata {
    /// Create new metadata with current timestamp
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            updated_at: now,
            version: 1,
            tags: None,
        }
    }

    /// Update the metadata (bumps version and updates timestamp)
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
        self.version += 1;
    }

    /// Add a tag
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        match &mut self.tags {
            Some(tags) => tags.push(tag.into()),
            None => self.tags = Some(vec![tag.into()]),
        }
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Core entity representing a tracked item
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// Unique identifier
    pub id: Id,
    /// Display name
    pub name: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Current status
    pub status: Status,
    /// Entity metadata
    pub metadata: Metadata,
}

impl Entity {
    /// Create a new entity with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Id::new(),
            name: name.into(),
            description: None,
            status: Status::default(),
            metadata: Metadata::new(),
        }
    }

    /// Set the description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the initial status
    pub fn with_status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    /// Update the entity name
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.metadata.touch();
    }

    /// Update the status
    pub fn set_status(&mut self, status: Status) {
        self.status = status;
        self.metadata.touch();
    }

    /// Check if the entity is active
    pub fn is_active(&self) -> bool {
        self.status == Status::Active
    }

    /// Check if the entity is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self.status, Status::Completed | Status::Failed | Status::Archived)
    }
}

/// Request to create a new entity
#[derive(Debug, Clone, Deserialize)]
pub struct CreateEntityRequest {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

impl CreateEntityRequest {
    /// Validate the request
    pub fn validate(&self) -> crate::Result<()> {
        if self.name.is_empty() {
            return Err(crate::error::MantaError::Validation(
                "Name cannot be empty".to_string(),
            ));
        }
        if self.name.len() > 256 {
            return Err(crate::error::MantaError::Validation(
                "Name cannot exceed 256 characters".to_string(),
            ));
        }
        Ok(())
    }

    /// Convert to an Entity
    pub fn into_entity(self) -> Entity {
        let mut entity = Entity::new(self.name);
        if let Some(desc) = self.description {
            entity.description = Some(desc);
        }
        if let Some(tags) = self.tags {
            entity.metadata.tags = Some(tags);
        }
        entity
    }
}

/// Request to update an existing entity
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateEntityRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<Status>,
    pub tags: Option<Vec<String>>,
}

impl UpdateEntityRequest {
    /// Apply updates to an entity
    pub fn apply(&self, entity: &mut Entity) -> crate::Result<()> {
        if let Some(name) = &self.name {
            if name.is_empty() {
                return Err(crate::error::MantaError::Validation(
                    "Name cannot be empty".to_string(),
                ));
            }
            entity.set_name(name.clone());
        }

        if let Some(desc) = &self.description {
            entity.description = Some(desc.clone());
            entity.metadata.touch();
        }

        if let Some(status) = self.status {
            entity.set_status(status);
        }

        if let Some(tags) = &self.tags {
            entity.metadata.tags = Some(tags.clone());
            entity.metadata.touch();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_creation() {
        let id1 = Id::new();
        let id2 = Id::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_id_parse() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let id = Id::parse(uuid_str).unwrap();
        assert_eq!(id.to_string(), uuid_str);
    }

    #[test]
    fn test_entity_lifecycle() {
        let mut entity = Entity::new("Test Entity").with_description("A test entity");

        assert_eq!(entity.name, "Test Entity");
        assert_eq!(entity.description, Some("A test entity".to_string()));
        assert_eq!(entity.status, Status::Pending);

        entity.set_status(Status::Active);
        assert_eq!(entity.status, Status::Active);
        assert!(entity.is_active());
        assert!(!entity.is_terminal());

        entity.set_status(Status::Completed);
        assert!(entity.is_terminal());
    }

    #[test]
    fn test_metadata_touch() {
        let mut meta = Metadata::new();
        let initial_version = meta.version;
        let initial_updated = meta.updated_at;

        std::thread::sleep(std::time::Duration::from_millis(10));
        meta.touch();

        assert_eq!(meta.version, initial_version + 1);
        assert!(meta.updated_at > initial_updated);
    }

    #[test]
    fn test_create_request_validation() {
        let valid = CreateEntityRequest {
            name: "Valid".to_string(),
            description: None,
            tags: None,
        };
        assert!(valid.validate().is_ok());

        let invalid = CreateEntityRequest {
            name: "".to_string(),
            description: None,
            tags: None,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_update_request_apply() {
        let mut entity = Entity::new("Original");
        let update = UpdateEntityRequest {
            name: Some("Updated".to_string()),
            description: Some("New desc".to_string()),
            status: Some(Status::Active),
            tags: Some(vec!["tag1".to_string()]),
        };

        update.apply(&mut entity).unwrap();

        assert_eq!(entity.name, "Updated");
        assert_eq!(entity.description, Some("New desc".to_string()));
        assert_eq!(entity.status, Status::Active);
    }
}
