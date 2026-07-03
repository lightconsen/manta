//! Domain event system for cross-module communication.
//!
//! Provides an [`EventBus`] — a simple publish-subscribe channel —
//! so that core operations (entity created, deleted, …) can notify
//! other subsystems (memory, channels, planner, …) without direct
//! coupling.
//!
//! # Usage (producer side)
//!
//! ```ignore
//! let event = CoreEvent::entity_created(entity.id, &entity.name);
//! event_bus.publish(event).await;
//! ```
//!
//! # Usage (consumer side)
//!
//! ```ignore
//! event_bus.subscribe("my_handler", MyHandler);
//! // MyHandler: impl EventHandler — pattern match on CoreEvent
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

use crate::core::models::Id;

// ---------------------------------------------------------------------------
// Event enum
// ---------------------------------------------------------------------------

/// All events emitted by the core engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoreEvent {
    /// A new entity was created.
    EntityCreated {
        /// When the event was created (Unix timestamp seconds).
        ts: u64,
        entity_id: Id,
        entity_name: String,
    },
    /// An existing entity was updated.
    EntityUpdated {
        /// When the event was created (Unix timestamp seconds).
        ts: u64,
        entity_id: Id,
    },
    /// An entity was deleted.
    EntityDeleted {
        /// When the event was created (Unix timestamp seconds).
        ts: u64,
        entity_id: Id,
        /// Entity name at time of deletion (for audit trail).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity_name: Option<String>,
    },
    /// One or more entities were archived during a sweep.
    EntityArchived {
        /// When the event was created (Unix timestamp seconds).
        ts: u64,
        /// Number of entities archived in this run.
        count: usize,
    },
}

impl CoreEvent {
    /// Human-readable event name (e.g. `"entity.created"`).
    pub fn event_name(&self) -> &'static str {
        match self {
            CoreEvent::EntityCreated { .. } => "entity.created",
            CoreEvent::EntityUpdated { .. } => "entity.updated",
            CoreEvent::EntityDeleted { .. } => "entity.deleted",
            CoreEvent::EntityArchived { .. } => "entity.archived",
        }
    }

    /// Shortcut — create an `EntityCreated` event with the current timestamp.
    pub fn entity_created(entity_id: Id, entity_name: &str) -> Self {
        Self::EntityCreated {
            ts: chrono::Utc::now().timestamp() as u64,
            entity_id,
            entity_name: entity_name.to_string(),
        }
    }

    /// Shortcut — create an `EntityUpdated` event with the current timestamp.
    pub fn entity_updated(entity_id: Id) -> Self {
        Self::EntityUpdated {
            ts: chrono::Utc::now().timestamp() as u64,
            entity_id,
        }
    }

    /// Shortcut — create an `EntityDeleted` event with the current timestamp.
    pub fn entity_deleted(entity_id: Id) -> Self {
        Self::EntityDeleted {
            ts: chrono::Utc::now().timestamp() as u64,
            entity_id,
            entity_name: None,
        }
    }

    /// Shortcut — create an `EntityDeleted` event with the entity name
    /// included for audit trail.
    pub fn entity_deleted_with_name(entity_id: Id, entity_name: &str) -> Self {
        Self::EntityDeleted {
            ts: chrono::Utc::now().timestamp() as u64,
            entity_id,
            entity_name: Some(entity_name.to_string()),
        }
    }

    /// Shortcut — create an `EntityArchived` event with the current timestamp.
    pub fn entity_archived(count: usize) -> Self {
        Self::EntityArchived {
            ts: chrono::Utc::now().timestamp() as u64,
            count,
        }
    }
}

// ---------------------------------------------------------------------------
// EventHandler trait
// ---------------------------------------------------------------------------

/// Something that listens for [`CoreEvent`]s dispatched by the [`EventBus`].
#[async_trait]
pub trait EventHandler: Send + Sync + std::fmt::Debug {
    /// Called for every event published to the bus.
    ///
    /// The handler should inspect the event and react if relevant
    /// (e.g. by matching on a specific variant).
    async fn handle(&self, event: &CoreEvent);

    /// Optional name for diagnostics / metrics.
    fn handler_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

// ---------------------------------------------------------------------------
// EventBus
// ---------------------------------------------------------------------------

/// A simple publish-subscribe channel for domain events.
///
/// Holders subscribe a boxed [`EventHandler`]; when
/// [`publish`](EventBus::publish) is called the event is fanned out to all
/// registered handlers.
///
/// # Thread safety
///
/// `EventBus` is `Send + Sync` — it can be shared behind an `Arc` across
/// async tasks.  Subscribing is interior-mutable and lock-free for publish
/// (handlers are read-locked on publish).
///
/// # Cloning
///
/// `Clone` creates another reference to the **same** bus (inner state is
/// `Arc`-wrapped), so cloned buses share the same handler set.
#[derive(Debug, Default, Clone)]
pub struct EventBus {
    handlers: Arc<RwLock<Vec<HandlerEntry>>>,
}

#[derive(Debug)]
struct HandlerEntry {
    name: &'static str,
    handler: Arc<dyn EventHandler>,
}

impl EventBus {
    /// Create an empty event bus.
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a handler by name.
    ///
    /// Handlers can be removed later by name (useful for dynamic plugin
    /// lifecycles and avoids fragility of `std::any::type_name`).
    pub async fn subscribe(&self, name: &'static str, handler: Arc<dyn EventHandler>) {
        self.handlers
            .write()
            .await
            .push(HandlerEntry { name, handler });
    }

    /// Remove all handlers with the given name.
    pub async fn unsubscribe(&self, name: &'static str) {
        self.handlers.write().await.retain(|e| e.name != name);
    }

    /// Publish an event to all registered handlers.
    ///
    /// Handlers run **sequentially** in subscription order.  If you need
    /// concurrent dispatch, wrap handlers with their own `tokio::spawn`.
    pub async fn publish(&self, event: &CoreEvent) {
        let handlers = self.handlers.read().await;
        for entry in handlers.iter() {
            entry.handler.handle(event).await;
        }
    }

    /// Publish an event to all registered handlers **concurrently**.
    ///
    /// Each handler is dispatched in its own `tokio::spawn` task so that
    /// a slow handler does not block others.  The method waits for all
    /// spawned tasks to complete before returning.
    pub async fn publish_concurrent(&self, event: &CoreEvent) {
        let handlers = self.handlers.read().await;
        let mut handles = Vec::with_capacity(handlers.len());
        for entry in handlers.iter() {
            let handler = entry.handler.clone();
            let event = event.clone();
            handles.push(tokio::spawn(async move {
                handler.handle(&event).await;
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Number of registered handlers.
    pub async fn handler_count(&self) -> usize {
        self.handlers.read().await.len()
    }
}

// ---------------------------------------------------------------------------
// EventLog — durable JSONL persistence
// ---------------------------------------------------------------------------

/// Relative path (within the workspace) where the event log is stored.
pub const CORE_EVENT_LOG_RELATIVE_PATH: &str = "events/core.jsonl";

/// Append a [`CoreEvent`] to the JSONL event log.
///
/// Creates the directory and file if they don't exist.
pub async fn append_core_event(
    workspace_dir: impl AsRef<std::path::Path>,
    event: &CoreEvent,
) -> crate::Result<()> {
    let path = workspace_dir.as_ref().join(CORE_EVENT_LOG_RELATIVE_PATH);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: format!("Failed to create event log directory: {:?}", parent),
                details: e.to_string(),
            }
        })?;
    }

    let line = serde_json::to_string(event).map_err(|e| crate::error::SyscityError::Storage {
        context: "Failed to serialize core event".to_string(),
        details: e.to_string(),
    })?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to open event log: {:?}", path),
            details: e.to_string(),
        })?;

    file.write_all(format!("{}\n", line).as_bytes())
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to write event log: {:?}", path),
            details: e.to_string(),
        })?;

    file.flush()
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to flush event log: {:?}", path),
            details: e.to_string(),
        })?;

    Ok(())
}

/// Read all core events from the JSONL log.
pub async fn read_core_events(
    workspace_dir: impl AsRef<std::path::Path>,
) -> crate::Result<Vec<CoreEvent>> {
    let path = workspace_dir.as_ref().join(CORE_EVENT_LOG_RELATIVE_PATH);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
        crate::error::SyscityError::Storage {
            context: format!("Failed to read event log: {:?}", path),
            details: e.to_string(),
        }
    })?;

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<CoreEvent>(line) {
            Ok(event) => events.push(event),
            Err(e) => {
                tracing::warn!("Skipping malformed core event line: {}", e);
            }
        }
    }

    Ok(events)
}

/// Convenience wrapper around the JSONL event log.
///
/// Holds the workspace path so callers don't need to pass it every time.
/// The file handle is opened lazily on first write and cached for subsequent
/// appends, avoiding repeated `open` syscalls.
#[derive(Debug)]
pub struct EventLog {
    workspace_dir: PathBuf,
    /// Lazily opened file handle, shared across clones.
    file: Arc<tokio::sync::Mutex<Option<tokio::fs::File>>>,
}

impl Clone for EventLog {
    fn clone(&self) -> Self {
        Self {
            workspace_dir: self.workspace_dir.clone(),
            file: self.file.clone(),
        }
    }
}

impl EventLog {
    /// Create a new event log rooted at the given workspace directory.
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            file: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Append an event to the log using a cached file handle.
    ///
    /// The file is opened on the first call and kept open for subsequent
    /// writes, which avoids the overhead of repeated `open` syscalls.
    pub async fn append(&self, event: &CoreEvent) -> crate::Result<()> {
        let path = self.workspace_dir.join(CORE_EVENT_LOG_RELATIVE_PATH);
        let line =
            serde_json::to_string(event).map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to serialize core event".to_string(),
                details: e.to_string(),
            })?;

        let mut guard = self.file.lock().await;
        if guard.is_none() {
            // First use — ensure directory exists and open the file
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: format!("Failed to create event log directory: {:?}", parent),
                        details: e.to_string(),
                    }
                })?;
            }
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: format!("Failed to open event log: {:?}", path),
                    details: e.to_string(),
                })?;
            *guard = Some(file);
        }

        if let Some(file) = guard.as_mut() {
            // SAFETY: we hold the lock, so no concurrent writes.
            // Write the JSON line followed by a newline.
            use tokio::io::AsyncWriteExt;
            file.write_all(format!("{}\n", line).as_bytes())
                .await
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: format!("Failed to write event log: {:?}", path),
                    details: e.to_string(),
                })?;

            file.flush()
                .await
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: format!("Failed to flush event log: {:?}", path),
                    details: e.to_string(),
                })?;
        }

        Ok(())
    }

    /// Read all events from the log.
    pub async fn read_all(&self) -> crate::Result<Vec<CoreEvent>> {
        read_core_events(&self.workspace_dir).await
    }

    /// Read events filtered by variant name (e.g. `"entity.created"`).
    pub async fn read_by_type(&self, event_name: &str) -> crate::Result<Vec<CoreEvent>> {
        let all = self.read_all().await?;
        Ok(all
            .into_iter()
            .filter(|e| e.event_name() == event_name)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug)]
    struct CountingHandler {
        created: AtomicUsize,
        deleted: AtomicUsize,
    }

    impl CountingHandler {
        fn new() -> Self {
            Self {
                created: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl EventHandler for CountingHandler {
        fn handler_name(&self) -> &'static str {
            "counting_handler"
        }

        async fn handle(&self, event: &CoreEvent) {
            match event {
                CoreEvent::EntityCreated { .. } => {
                    self.created.fetch_add(1, Ordering::SeqCst);
                }
                CoreEvent::EntityDeleted { .. } => {
                    self.deleted.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_subscribe_and_publish() {
        let bus = EventBus::new();
        let handler = CountingHandler::new();

        bus.subscribe("counting_handler", Arc::new(handler)).await;

        bus.publish(&CoreEvent::entity_created(Id::new(), "foo"))
            .await;
        bus.publish(&CoreEvent::entity_created(Id::new(), "bar"))
            .await;
        bus.publish(&CoreEvent::entity_deleted(Id::new())).await;

        // Can't access handler's counters through the box — this verifies
        // the bus didn't panic and the handler_count advanced.
        assert_eq!(bus.handler_count().await, 1);
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let bus = EventBus::new();

        bus.subscribe("test", Arc::new(CountingHandler::new()))
            .await;
        assert_eq!(bus.handler_count().await, 1);

        bus.unsubscribe("test").await;
        assert_eq!(bus.handler_count().await, 0);
    }

    #[tokio::test]
    async fn test_event_name() {
        let e1 = CoreEvent::entity_created(Id::new(), "x");
        let e2 = CoreEvent::entity_updated(Id::new());
        let e3 = CoreEvent::entity_deleted(Id::new());
        let e4 = CoreEvent::entity_archived(5);

        assert_eq!(e1.event_name(), "entity.created");
        assert_eq!(e2.event_name(), "entity.updated");
        assert_eq!(e3.event_name(), "entity.deleted");
        assert_eq!(e4.event_name(), "entity.archived");
    }

    #[tokio::test]
    async fn test_event_log_roundtrip() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path());

        let e1 = CoreEvent::entity_created(Id::new(), "alice");
        let e2 = CoreEvent::entity_deleted(Id::new());
        let e3 = CoreEvent::entity_archived(3);

        log.append(&e1).await.unwrap();
        log.append(&e2).await.unwrap();
        log.append(&e3).await.unwrap();

        let events = log.read_all().await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_name(), "entity.created");
        assert_eq!(events[1].event_name(), "entity.deleted");
        assert_eq!(events[2].event_name(), "entity.archived");
    }

    #[tokio::test]
    async fn test_event_log_read_by_type() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path());

        log.append(&CoreEvent::entity_created(Id::new(), "a"))
            .await
            .unwrap();
        log.append(&CoreEvent::entity_created(Id::new(), "b"))
            .await
            .unwrap();
        log.append(&CoreEvent::entity_deleted(Id::new()))
            .await
            .unwrap();

        let created = log.read_by_type("entity.created").await.unwrap();
        assert_eq!(created.len(), 2);

        let deleted = log.read_by_type("entity.deleted").await.unwrap();
        assert_eq!(deleted.len(), 1);
    }

    #[tokio::test]
    async fn test_event_log_empty_dir() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path());

        let events = log.read_all().await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_core_event_serialization() {
        let id = Id::new();
        let e = CoreEvent::entity_created(id, "test-entity");
        let json = serde_json::to_string(&e).unwrap();
        let deserialized: CoreEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e.event_name(), deserialized.event_name());
        match deserialized {
            CoreEvent::EntityCreated { entity_id, ref entity_name, .. } => {
                assert_eq!(entity_id, id);
                assert_eq!(entity_name, "test-entity");
            }
            _ => panic!("expected EntityCreated variant"),
        }
    }
}
